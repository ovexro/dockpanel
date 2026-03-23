use axum::{
    extract::Path,
    http::StatusCode,
    routing::{delete, get, post},
    Json, Router,
};

use super::{is_valid_name, AppState};
use crate::services::{backups::BackupInfo, database_backup, encryption};

type ApiErr = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: &str) -> ApiErr {
    (status, Json(serde_json::json!({ "error": msg })))
}

#[derive(serde::Deserialize)]
pub struct DumpRequest {
    pub container_name: String,
    pub db_name: String,
    pub db_type: String,
    pub user: String,
    pub password: String,
    pub encryption_key: Option<String>,
}

#[derive(serde::Deserialize)]
pub struct RestoreRequest {
    pub container_name: String,
    pub db_type: String,
    pub user: String,
    pub password: String,
    pub encryption_key: Option<String>,
}

/// POST /db-backups/dump — Dump a database.
async fn dump(
    Json(req): Json<DumpRequest>,
) -> Result<Json<BackupInfo>, ApiErr> {
    if !is_valid_name(&req.db_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid database name"));
    }
    if !is_valid_name(&req.container_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }
    if !is_valid_name(&req.user) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid user name"));
    }
    if req.password.contains('\0') || req.password.contains('\n') || req.password.contains('\r') || req.password.len() > 128 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid password"));
    }

    let mut info = match req.db_type.as_str() {
        "mysql" | "mariadb" => {
            database_backup::dump_mysql(&req.container_name, &req.db_name, &req.user, &req.password)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        }
        "postgres" | "postgresql" => {
            database_backup::dump_postgres(&req.container_name, &req.db_name, &req.user, &req.password)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        }
        "mongo" | "mongodb" => {
            database_backup::dump_mongo(&req.container_name, &req.db_name)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        }
        _ => return Err(err(StatusCode::BAD_REQUEST, "Unsupported database type")),
    };

    // Encrypt if key provided
    if let Some(key) = &req.encryption_key {
        if !key.is_empty() {
            let filepath = database_backup::get_backup_path(&req.db_name, &info.filename)
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
            let enc_path = encryption::encrypt_file(&filepath, key)
                .await
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
            let enc_meta = std::fs::metadata(&enc_path)
                .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Encrypted file: {e}")))?;
            info.filename = format!("{}.enc", info.filename);
            info.size_bytes = enc_meta.len();
        }
    }

    Ok(Json(info))
}

/// GET /db-backups/{db_name}/list — List database backups.
async fn list(
    Path(db_name): Path<String>,
) -> Result<Json<Vec<BackupInfo>>, ApiErr> {
    if !is_valid_name(&db_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid database name"));
    }

    let list = database_backup::list_db_backups(&db_name)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(list))
}

/// DELETE /db-backups/{db_name}/{filename} — Delete a database backup.
async fn remove(
    Path((db_name, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_name(&db_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid database name"));
    }

    database_backup::delete_db_backup(&db_name, &filename)
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

/// POST /db-backups/{db_name}/restore/{filename} — Restore a database from backup.
async fn restore(
    Path((db_name, filename)): Path<(String, String)>,
    Json(req): Json<RestoreRequest>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_name(&db_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid database name"));
    }
    if !is_valid_name(&req.container_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid container name"));
    }
    if !is_valid_name(&req.user) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid user name"));
    }
    if req.password.contains('\0') || req.password.contains('\n') || req.password.contains('\r') || req.password.len() > 128 {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid password"));
    }

    // Resolve and validate backup file path
    let filepath = database_backup::get_backup_path(&db_name, &filename)
        .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;

    // If the file is encrypted, decrypt it first
    let restore_path = if filepath.ends_with(".enc") {
        let key = req.encryption_key.as_deref()
            .ok_or_else(|| err(StatusCode::BAD_REQUEST, "Encryption key required for encrypted backup"))?;
        if key.is_empty() {
            return Err(err(StatusCode::BAD_REQUEST, "Encryption key required for encrypted backup"));
        }
        encryption::decrypt_file(&filepath, key)
            .await
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?
    } else {
        filepath.clone()
    };

    let result = match req.db_type.as_str() {
        "mysql" | "mariadb" => {
            database_backup::restore_mysql(
                &req.container_name, &db_name, &req.user, &req.password, &restore_path,
            ).await
        }
        "postgres" | "postgresql" => {
            database_backup::restore_postgres(
                &req.container_name, &db_name, &req.user, &req.password, &restore_path,
            ).await
        }
        "mongo" | "mongodb" => {
            database_backup::restore_mongo(
                &req.container_name, &db_name, &restore_path,
            ).await
        }
        _ => Err("Unsupported database type".to_string()),
    };

    // Clean up decrypted temp file if we created one
    if restore_path != filepath {
        std::fs::remove_file(&restore_path).ok();
    }

    result.map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    Ok(Json(serde_json::json!({
        "success": true,
        "db_name": db_name,
        "filename": filename,
        "db_type": req.db_type,
    })))
}

/// GET /db-backups/{db_name}/{filename}/path — Get filesystem path for upload.
async fn get_path(
    Path((db_name, filename)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiErr> {
    if !is_valid_name(&db_name) {
        return Err(err(StatusCode::BAD_REQUEST, "Invalid database name"));
    }

    let path = database_backup::get_backup_path(&db_name, &filename)
        .map_err(|e| err(StatusCode::NOT_FOUND, &e))?;
    Ok(Json(serde_json::json!({ "path": path })))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/db-backups/dump", post(dump))
        .route("/db-backups/{db_name}/list", get(list))
        .route("/db-backups/{db_name}/{filename}", delete(remove))
        .route("/db-backups/{db_name}/{filename}/path", get(get_path))
        .route("/db-backups/{db_name}/restore/{filename}", post(restore))
}
