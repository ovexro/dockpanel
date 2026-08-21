use axum::{http::StatusCode, Json};

use crate::services::agent::AgentError;

pub type ApiError = (StatusCode, Json<serde_json::Value>);

/// Stable machine-readable marker for "the agent could not be reached at all".
///
/// Clients must branch on this rather than on the bare HTTP status. A 502 from
/// this panel can mean *the agent never answered* or *the agent answered and
/// something failed inside it*, and only the first justifies telling an
/// operator their agent is offline.
pub const CODE_AGENT_UNREACHABLE: &str = "agent_unreachable";

/// Stable machine-readable marker for "re-present a credential before this
/// account may enrol a new authenticator".
///
/// The status carried alongside it is deliberately **403, never 401**, and it
/// stays 403 now that [`CODE_SESSION_INVALID`] has made 401 survivable. The
/// session is still valid; only this one action is refused, which is what 403
/// means. Choosing it is no longer a way around the client — it is the correct
/// status — so do not "restore" this to 401 on the grounds that bodies now
/// survive.
pub const CODE_REAUTH_REQUIRED: &str = "reauth_required";

/// Stable machine-readable marker for "the caller has no usable session".
///
/// This is the ONLY thing that entitles the frontend to navigate someone to
/// `/login`, and it is emitted from exactly one place: the [`AuthUser`]
/// extractor and its `ServerScope` sibling in `crate::auth`. The rule behind
/// that is structural rather than a matter of taste — **if a handler returned
/// 401, the session was valid**, because the extractor had already let the
/// request through. A handler's 401 means the credential presented *inside* an
/// authenticated request was refused: a wrong password, a wrong TOTP code, a
/// signature that did not verify. Logging someone out for that is answering a
/// question nobody asked.
///
/// Until v2.87.0 the client could not tell the two apart, so it assumed the
/// worst about all of them: it navigated to `/login` and threw a fixed
/// "Unauthorized" *before the response body was ever parsed*. Every sentence
/// written at a 401 site in this codebase — including the one telling a user
/// their administrator had reset their 2FA — was discarded unread.
///
/// [`AuthUser`]: crate::auth::AuthUser
pub const CODE_SESSION_INVALID: &str = "session_invalid";

/// Stable machine-readable marker for "what you sent will not fit through the
/// panel".
///
/// It exists because the framework's own refusal cannot be rendered. An
/// oversize body is rejected by axum's request-body limit before any handler
/// runs, as `413` with the plain-text sentence "Failed to buffer the request
/// body: length limit exceeded" — which `api.ts` correctly degrades to
/// "Request failed (413)", a sentence that tells an operator nothing. Issue
/// #121 was filed because a 33 MB upload produced no explanation at all.
/// Handlers that can be handed an oversize body take the rejection instead of
/// letting it short-circuit, and answer with this code and a real sentence.
pub const CODE_PAYLOAD_TOO_LARGE: &str = "payload_too_large";

/// Longest agent-authored message passed through to a client.
const MAX_AGENT_MSG: usize = 400;

/// The label the agent puts on a certificate order that failed validation.
///
/// ⚠ The agent crate carries this same literal and cannot share a constant with
/// this one — two crates, no common dependency. That makes it a severed pair by
/// construction, so a pin compares the two spellings across both trees rather
/// than trusting either alone; renaming it here and nowhere else silently
/// returns every failed order to an incident id, with nothing red.
const ACME_ORDER_FAILED_CODE: &str = "acme_order_failed";

/// The label the agent puts on a DNS-01 challenge the DNS PROVIDER refused.
///
/// The third party on the third door. A Cloudflare token without `DNS:Edit`
/// cannot publish `_acme-challenge`, and that is the operator's to fix — but it
/// is not something the certificate authority said, so it may not borrow
/// [`ACME_ORDER_FAILED_CODE`]'s sentence, which names the CA. Same severed-pair
/// caveat as its sibling, same cross-tree pin.
const DNS_PROVIDER_FAILED_CODE: &str = "dns_provider_failed";

/// The renewal route's wrapper around the same failure, carried by every agent
/// ever shipped. Recognising it is what lets an install nobody has updated get
/// the honest answer today. Same severed-pair caveat, same pin.
const RENEWAL_FAILURE_PREFIX: &str = "Renewal failed:";

pub fn err(status: StatusCode, msg: &str) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg })))
}

/// [`err`] plus a stable `code` the frontend can branch on.
pub fn err_coded(status: StatusCode, msg: &str, code: &str) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg, "code": code })))
}

/// Extract the human-facing sentence from an agent error body.
///
/// The agent answers errors in three shapes — `{"error": …}`, `{"success":
/// false, "message": …}`, and occasionally bare text — so all three are
/// handled rather than the one that happened to be in front of us.
fn agent_message(body: &str) -> String {
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .or_else(|| v.get("message"))
                .and_then(|m| m.as_str().map(String::from))
        })
        .unwrap_or_else(|| body.to_string());

    // Don't surface the Docker daemon's framing around an inner diagnostic.
    let msg = msg
        .strip_prefix("Error response from daemon: ")
        .unwrap_or(&msg)
        .trim()
        .to_string();

    if msg.chars().count() > MAX_AGENT_MSG {
        let truncated: String = msg.chars().take(MAX_AGENT_MSG).collect();
        format!("{truncated}…")
    } else {
        msg
    }
}

/// The stable `code` an agent attached to an error body, if it attached one.
///
/// Shared by the two label readers so the extraction lives once — a second
/// hand-rolled copy is how one of them silently stops matching.
fn agent_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("code").and_then(|c| c.as_str()).map(String::from))
}

/// The sentence a DNS provider's refusal should reach the operator as, or `None`
/// when this failure is not one.
///
/// The label is the ONLY proof accepted here, and deliberately so. Its sibling
/// [`acme_order_failure`] has a second one — the renewal route's wrapper, carried
/// by every agent ever shipped — which is what lets an un-updated install get the
/// honest answer today. The DNS-01 door never had such a wrapper, so there is no
/// legacy shape to recognise, and inventing one out of the agent's old message
/// prefixes would be guessing at text this very change rewrites. An agent older
/// than this panel keeps its incident id here, which is what it is.
pub fn dns_provider_failure(e: &AgentError) -> Option<String> {
    let AgentError::Status(code, body) = e else {
        return None;
    };
    if !(500..600).contains(code) {
        return None;
    }
    if agent_code(body).is_some_and(|c| c == DNS_PROVIDER_FAILED_CODE) {
        Some(agent_message(body).trim().to_string())
    } else {
        None
    }
}

/// The sentence an agent's failed ACME order should reach the operator as, or
/// `None` when this failure is not one.
///
/// A certificate order that fails validation is the operator's to fix — the
/// domain does not resolve here, or port 80 is shut, or a wildcard's apex lives
/// somewhere else. It is the single most likely way an SSL button fails, and
/// [`agent_error`] answers all of it with an incident id, because the agent
/// reports it as a 500 and every 5xx is hidden by design. The rule is right;
/// this case is the exception, and it has to be identified POSITIVELY rather
/// than by widening the rule, because the same route answers 500 for a genuine
/// internal fault (an unreadable ACME account) that an incident id describes
/// correctly.
///
/// Two ways to be sure, and the order matters:
///
/// 1. The agent labels the order failure with a stable code. That is exact, and
///    it works at every door.
/// 2. Failing that, the renewal route's own wrapper text. This is the half that
///    reaches installs NOBODY HAS UPDATED — an agent is only updated when
///    somebody updates it, so a fix that lives only in the agent does not
///    arrive, which is the whole reason the s386 Fix button was repaired
///    panel-side. Provisioning has no such wrapper, so an older agent keeps its
///    incident id there rather than being guessed at.
///
/// Anything else stays a 502 with a reference, which is what it is.
pub fn acme_order_failure(e: &AgentError) -> Option<String> {
    let AgentError::Status(code, body) = e else {
        return None;
    };
    if !(500..600).contains(code) {
        return None;
    }
    let labelled = agent_code(body).is_some_and(|c| c == ACME_ORDER_FAILED_CODE);

    let msg = agent_message(body);
    if labelled || msg.starts_with(RENEWAL_FAILURE_PREFIX) {
        Some(
            msg.strip_prefix(RENEWAL_FAILURE_PREFIX)
                .unwrap_or(&msg)
                .trim()
                .to_string(),
        )
    } else {
        None
    }
}

/// The status and sentence an agent failure should reach the caller as, or
/// `None` when it is one only an incident id should describe.
///
/// [`agent_error`] below is the whole answer whenever ONE agent call decides the
/// response. A caller that asks SEVERAL members and has to summarise them cannot
/// use it — it holds N errors and must pick one status — and the way that has
/// gone wrong is by stringifying each error and hand-rolling a 502, which throws
/// away precisely the 4xx this module exists to preserve. Exposing the rule lets
/// a fan-out apply it per member instead of re-deriving it, and keeps both the
/// client-error boundary and the message extraction in one place, not two.
pub fn agent_actionable(e: &AgentError) -> Option<(StatusCode, String)> {
    match e {
        AgentError::Status(code, body) if (400..500).contains(code) => Some((
            StatusCode::from_u16(*code).unwrap_or(StatusCode::BAD_REQUEST),
            agent_message(body),
        )),
        _ => None,
    }
}

/// Convert an agent-call failure into a client-facing error.
///
/// The distinction this function exists to preserve: a **4xx from the agent is
/// an answer**, authored deliberately for whoever asked ("Path must be within
/// /var/backups/", "WAF not installed"), and collapsing it into a
/// generic 502 tells the operator their agent is down when it is working
/// perfectly. Only a transport failure, an open circuit breaker, or a genuine
/// 5xx from inside the agent is worth hiding behind an incident id.
///
/// Takes `AgentError` by value rather than `impl Display` on purpose: the
/// status is structured, and a `Display` bound is what let it be stringified
/// and thrown away at 244 call sites.
pub fn agent_error(context: &str, e: AgentError) -> ApiError {
    match e {
        // The agent refused, and said why. Pass both through.
        AgentError::Status(code, ref body) if (400..500).contains(&code) => {
            let status =
                StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            let msg = agent_message(body);
            tracing::warn!(agent_status = code, error = %msg, "{context}");
            err(status, &msg)
        }

        // The agent broke internally. Log it, hand back a reference.
        AgentError::Status(code, ref body) => {
            let incident_id = uuid::Uuid::new_v4();
            tracing::error!(
                incident_id = %incident_id,
                agent_status = code,
                error = %agent_message(body),
                "{context}"
            );
            err(
                StatusCode::BAD_GATEWAY,
                &format!("Operation failed. Reference: {incident_id}"),
            )
        }

        AgentError::NotFound(ref detail) => {
            tracing::warn!(error = %detail, "{context}");
            err(StatusCode::NOT_FOUND, detail)
        }

        // Nothing answered. This is the only case that means "agent offline".
        AgentError::Connection(_) | AgentError::CircuitOpen(_) => {
            let incident_id = uuid::Uuid::new_v4();
            tracing::error!(incident_id = %incident_id, error = %e, "{context}");
            err_coded(
                StatusCode::BAD_GATEWAY,
                &format!("The agent is not responding. Reference: {incident_id}"),
                CODE_AGENT_UNREACHABLE,
            )
        }

        AgentError::Request(_) | AgentError::Response(_) | AgentError::Parse(_) => {
            let incident_id = uuid::Uuid::new_v4();
            tracing::error!(incident_id = %incident_id, error = %e, "{context}");
            err(
                StatusCode::BAD_GATEWAY,
                &format!("Operation failed. Reference: {incident_id}"),
            )
        }
    }
}

/// A call to a THIRD-PARTY service (Stripe, Cloudflare, …) failed.
///
/// Deliberately separate from [`agent_error`]: these calls never touch the
/// DockPanel agent, and routing them through the agent path is what made a
/// Cloudflare outage tell an operator their agent was offline. Names the
/// service so the log says which upstream broke, and carries no
/// `agent_unreachable` marker.
pub fn upstream_error(service: &str, e: impl std::fmt::Display) -> ApiError {
    let incident_id = uuid::Uuid::new_v4();
    tracing::error!(incident_id = %incident_id, error = %e, "{service}");
    err(
        StatusCode::BAD_GATEWAY,
        &format!("{service} failed. Reference: {incident_id}"),
    )
}

/// Log the real error internally but return only a generic message with a short reference ID.
/// Use for INTERNAL_SERVER_ERROR responses to prevent leaking DB schema, file paths, SQL, etc.
pub fn internal_error(context: &str, e: impl std::fmt::Display) -> ApiError {
    let incident_id = uuid::Uuid::new_v4();
    tracing::error!(incident_id = %incident_id, error = %e, "{context}");
    err(StatusCode::INTERNAL_SERVER_ERROR, &format!("Internal error (ref: {})", &incident_id.to_string()[..8]))
}

pub fn require_admin(role: &str) -> Result<(), ApiError> {
    if role != "admin" {
        Err(err(StatusCode::FORBIDDEN, "Admin access required"))
    } else {
        Ok(())
    }
}

pub fn paginate(limit: Option<i64>, offset: Option<i64>) -> (i64, i64) {
    let limit = limit.unwrap_or(100).max(1).min(200);
    let offset = offset.unwrap_or(0).max(0);
    (limit, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_of(e: &ApiError) -> serde_json::Value {
        e.1 .0.clone()
    }

    #[test]
    fn a_failed_order_is_recognised_by_its_label() {
        // ⚠ PROVISION-SHAPED: labelled, and NO renewal wrapper. The first version
        // of this fixture carried both, so it passed on the legacy-prefix arm alone
        // and the label test could have been deleted with everything still green.
        // This is the busier door's real shape and the only thing that exercises it.
        let e = AgentError::Status(
            500,
            r#"{"error":"The CA did not validate the challenge: no valid A record","code":"acme_order_failed"}"#.into(),
        );
        let reason = acme_order_failure(&e).expect("a labelled order failure");
        assert_eq!(reason, "The CA did not validate the challenge: no valid A record");
    }

    #[test]
    fn a_failed_order_is_recognised_on_an_agent_nobody_has_updated() {
        // ⭐ THE HALF THAT ARRIVES. No label, because the agent predates it — the
        // renewal route's own wording is the evidence, and this is the only reason
        // an existing install stops answering a reference number today.
        let e = AgentError::Status(
            500,
            r#"{"error":"Renewal failed: challenge did not validate"}"#.into(),
        );
        assert_eq!(
            acme_order_failure(&e).as_deref(),
            Some("challenge did not validate")
        );
    }

    #[test]
    fn an_internal_agent_fault_is_still_only_an_incident_id() {
        // ⭐ THE ASSERTION THAT STOPS THE FIX OVERREACHING — and it deliberately
        // uses a fault raised INSIDE the labelled call, not one raised before it.
        // The first version of this test used the ACME account load, which happens
        // above the labelled call and so could never have carried a label: it
        // could not have failed however wrong the classification was. A full disk
        // during the certificate write is the case that actually needs proving.
        let e = AgentError::Status(
            500,
            r#"{"error":"Failed to write cert: No space left on device (os error 28)"}"#.into(),
        );
        assert!(acme_order_failure(&e).is_none());
    }

    #[test]
    fn a_local_fault_on_the_renew_door_of_an_updated_agent_keeps_its_incident_id() {
        // An agent new enough to label puts the renewal wrapper on local faults
        // too, so the wrapper alone must not be enough when the label is absent
        // AND the agent is known to label. It is — the fallback is deliberately
        // wide, and this records what it costs: only an agent too old to label
        // reaches it. Documented in acme_order_failure's own note.
        let e = AgentError::Status(
            500,
            r#"{"error":"Renewal failed: Failed to write cert: No space left on device"}"#.into(),
        );
        assert!(acme_order_failure(&e).is_some());
    }

    #[test]
    fn a_4xx_is_not_an_order_failure_even_if_it_says_so() {
        // 4xx already passes through with its own status; taking it here would
        // rewrite a 400 into a 422.
        let e = AgentError::Status(
            400,
            r#"{"error":"Renewal failed: bad domain","code":"acme_order_failed"}"#.into(),
        );
        assert!(acme_order_failure(&e).is_none());
    }

    #[test]
    fn a_transport_failure_is_not_an_order_failure() {
        assert!(acme_order_failure(&AgentError::Connection("refused".into())).is_none());
    }

    #[test]
    fn agent_4xx_keeps_its_status_and_says_why() {
        let e = agent_error(
            "Backup analysis",
            AgentError::Status(
                400,
                r#"{"error":"Path must be within /var/backups/"}"#.into(),
            ),
        );
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
        assert_eq!(
            body_of(&e)["error"],
            "Path must be within /var/backups/"
        );
        // A refusal is not an unreachable agent.
        assert!(body_of(&e).get("code").is_none());
    }

    #[test]
    fn agent_412_is_preserved_not_flattened_to_502() {
        let e = agent_error(
            "WAF configure",
            AgentError::Status(
                412,
                r#"{"error":"WAF not installed. Install it from the Services page first."}"#.into(),
            ),
        );
        assert_eq!(e.0, StatusCode::PRECONDITION_FAILED);
        assert!(body_of(&e)["error"]
            .as_str()
            .unwrap()
            .contains("WAF not installed"));
    }

    #[test]
    fn success_false_message_shape_is_understood() {
        let e = agent_error(
            "WAF nginx config",
            AgentError::Status(
                400,
                r#"{"success":false,"message":"Nginx config test failed: unicode.mapping"}"#.into(),
            ),
        );
        assert_eq!(e.0, StatusCode::BAD_REQUEST);
        assert!(body_of(&e)["error"]
            .as_str()
            .unwrap()
            .contains("Nginx config test failed"));
    }

    #[test]
    fn agent_5xx_stays_generic_with_an_incident_id() {
        let e = agent_error(
            "Restore",
            AgentError::Status(500, r#"{"error":"/etc/dockpanel/secret.key missing"}"#.into()),
        );
        assert_eq!(e.0, StatusCode::BAD_GATEWAY);
        let msg = body_of(&e)["error"].as_str().unwrap().to_string();
        assert!(msg.starts_with("Operation failed. Reference: "));
        assert!(!msg.contains("secret.key"));
    }

    #[test]
    fn only_a_transport_failure_is_marked_unreachable() {
        let e = agent_error("Anything", AgentError::Connection("no such file".into()));
        assert_eq!(e.0, StatusCode::BAD_GATEWAY);
        assert_eq!(body_of(&e)["code"], CODE_AGENT_UNREACHABLE);

        let e = agent_error("Anything", AgentError::CircuitOpen("5 failures".into()));
        assert_eq!(body_of(&e)["code"], CODE_AGENT_UNREACHABLE);

        // A parse failure is ours, not evidence the agent is down.
        let e = agent_error("Anything", AgentError::Parse("bad json".into()));
        assert!(body_of(&e).get("code").is_none());
    }

    #[test]
    fn a_bare_text_body_survives() {
        let e = agent_error("Anything", AgentError::Status(404, "no such site".into()));
        assert_eq!(e.0, StatusCode::NOT_FOUND);
        assert_eq!(body_of(&e)["error"], "no such site");
    }

    #[test]
    fn a_long_agent_message_is_bounded() {
        let long = "x".repeat(MAX_AGENT_MSG * 3);
        let e = agent_error("Anything", AgentError::Status(400, long));
        let msg = body_of(&e)["error"].as_str().unwrap().to_string();
        assert!(msg.chars().count() <= MAX_AGENT_MSG + 1);
    }

    #[test]
    fn the_three_markers_are_distinct_strings() {
        // They mean three different things to the client — "your agent is
        // down", "re-present a credential", "log in again" — and a collision
        // would silently give one the other's behaviour.
        assert_ne!(CODE_SESSION_INVALID, CODE_REAUTH_REQUIRED);
        assert_ne!(CODE_SESSION_INVALID, CODE_AGENT_UNREACHABLE);
        assert_ne!(CODE_REAUTH_REQUIRED, CODE_AGENT_UNREACHABLE);
    }

    #[test]
    fn err_coded_keeps_the_sentence_beside_the_marker() {
        let e = err_coded(
            StatusCode::UNAUTHORIZED,
            "Session revoked. Please log in again.",
            CODE_SESSION_INVALID,
        );
        assert_eq!(e.0, StatusCode::UNAUTHORIZED);
        assert_eq!(body_of(&e)["code"], CODE_SESSION_INVALID);
        assert_eq!(body_of(&e)["error"], "Session revoked. Please log in again.");
    }

    #[test]
    fn a_bare_err_carries_no_marker() {
        // Every credential refusal in the codebase is minted with `err`. If
        // this ever grew a default code, every one of them would start
        // logging users out again.
        let e = err(StatusCode::UNAUTHORIZED, "Current password is incorrect");
        assert!(body_of(&e).get("code").is_none());
    }

    #[test]
    fn a_dns_provider_refusal_is_recognised_by_its_own_label() {
        // The fixture carries the provider label and NOTHING else — no renewal
        // wrapper, no ACME label — so exactly one branch can explain it (#666).
        let e = AgentError::Status(
            500,
            r#"{"error":"Cloudflare refused to create the challenge record: [{\"code\":10000}]","code":"dns_provider_failed"}"#.into(),
        );
        assert_eq!(
            dns_provider_failure(&e).as_deref(),
            Some("Cloudflare refused to create the challenge record: [{\"code\":10000}]")
        );
        // ⭐ THE ASSERTION THAT MATTERS: the CA reader must NOT claim it. If it
        // did, the panel would tell an operator Let's Encrypt declined when what
        // actually happened is their API token cannot write DNS.
        assert!(acme_order_failure(&e).is_none());
    }

    #[test]
    fn a_declined_order_is_not_a_provider_refusal() {
        // The mirror of the above, and the reason both are here: a classifier
        // proved only in one direction cannot be shown to discriminate.
        let e = AgentError::Status(
            500,
            r#"{"error":"The CA did not validate the DNS-01 challenge: incorrect TXT value","code":"acme_order_failed"}"#.into(),
        );
        assert!(dns_provider_failure(&e).is_none());
        assert!(acme_order_failure(&e).is_some());
    }

    #[test]
    fn the_renewal_wrapper_is_not_a_provider_refusal() {
        // The compatibility path belongs to the CA reader alone. The provider
        // reader takes the label and nothing else, so an un-updated agent keeps
        // its incident id here rather than being guessed at.
        let e = AgentError::Status(
            500,
            r#"{"error":"Renewal failed: challenge did not validate"}"#.into(),
        );
        assert!(dns_provider_failure(&e).is_none());
    }

    #[test]
    fn an_unlabelled_dns01_fault_is_still_only_an_incident_id() {
        // A fault raised INSIDE the labelled call — the certificate write, not
        // the account load above it — so it is a fault that COULD have carried a
        // label and deliberately does not.
        let e = AgentError::Status(
            500,
            r#"{"error":"Write cert: No space left on device (os error 28)"}"#.into(),
        );
        assert!(dns_provider_failure(&e).is_none());
        assert!(acme_order_failure(&e).is_none());
    }

    #[test]
    fn the_docker_daemon_prefix_is_stripped() {
        let e = agent_error(
            "Anything",
            AgentError::Status(
                409,
                r#"{"error":"Error response from daemon: name already in use"}"#.into(),
            ),
        );
        assert_eq!(body_of(&e)["error"], "name already in use");
    }
}
