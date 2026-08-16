// DockPanel chain-of-trust report.
// Compiled by services/chain_report.rs:render_chain_report_pdf via the typst
// CLI. Reads its data from sys.inputs.data_path (a JSON file).

#let data = json(sys.inputs.data_path)

#set document(
  title: "DockPanel Chain-of-Trust Report",
  author: "DockPanel " + data.panel_version,
)
#set page(
  paper: "us-letter",
  margin: (x: 0.75in, y: 0.85in),
  numbering: "1 / 1",
  number-align: center,
  footer: context [
    #set text(size: 8pt, fill: rgb("#777"))
    #grid(columns: (1fr, 1fr),
      align(left)[Generated #data.generated_at by DockPanel #data.panel_version],
      align(right)[Page #counter(page).display() of #counter(page).final().first()],
    )
  ],
)
#set text(font: "Liberation Sans", size: 10pt)
#show heading.where(level: 1): set text(size: 18pt, weight: "bold")
#show heading.where(level: 2): set text(size: 13pt, weight: "bold")

#let dp_brand = rgb("#d97706")
#let dp_pass = rgb("#059669")
#let dp_fail = rgb("#dc2626")
#let dp_pending = rgb("#a16207")
#let dp_dim = rgb("#6b7280")

#let status_pill(status) = {
  let (label, fill) = if status == "passed" { ("PASSED", dp_pass) }
    else if status == "failed" { ("FAILED", dp_fail) }
    else if status == "running" { ("RUNNING", dp_pending) }
    else if status == "pending" { ("PENDING", dp_pending) }
    else { (upper(status), dp_dim) }
  box(
    inset: (x: 6pt, y: 2pt),
    radius: 3pt,
    fill: fill,
    text(fill: white, size: 8pt, weight: "bold")[#label],
  )
}

#let kv(label, value) = grid(
  columns: (1.6in, 1fr),
  text(fill: dp_dim, size: 9pt)[#label],
  text(size: 10pt)[#value],
)

#let mono(s) = text(font: "Liberation Mono", size: 8.5pt)[#s]

#let humanbytes(n) = {
  if n < 1024 [#n B]
  else if n < 1024 * 1024 [#calc.round(n / 1024.0, digits: 1) KB]
  else if n < 1024 * 1024 * 1024 [#calc.round(n / 1024.0 / 1024.0, digits: 1) MB]
  else [#calc.round(n / 1024.0 / 1024.0 / 1024.0, digits: 2) GB]
}

#let duration_ms(ms) = if ms == none [—] else [#calc.round(ms / 1000.0, digits: 1) s]

// ── Header ──────────────────────────────────────────────────────────────

// The braces are load-bearing, not style. A `#`-expression written in MARKUP mode
// ends at the first line break that leaves it syntactically complete, so a bare
// `if …{}` here bound only the first branch and the `else if` lines below it were
// re-lexed as a paragraph and PRINTED above the masthead. Wrapping the chain in a
// code block makes newlines trivia, exactly as `humanbytes` and `status_pill` above
// already do — they were never broken for that reason alone.
#let kind_label = {
  if data.backup.kind == "site" { "Site" }
  else if data.backup.kind == "database" { "Database" }
  else if data.backup.kind == "volume" { "Volume" }
  else { upper(data.backup.kind) }
}

#align(left)[
  #text(size: 22pt, weight: "bold", fill: dp_brand)[DockPanel]
  #h(0.5em)
  #text(size: 14pt, fill: dp_dim, weight: "regular")[Chain-of-Trust Report]
  #h(0.5em)
  #text(size: 11pt, fill: dp_brand, weight: "bold")[· #upper(data.backup.kind)]
]

#v(0.5em)
#line(length: 100%, stroke: 0.5pt + dp_dim)
#v(0.3em)

#kv(kind_label, data.backup.resource_name)
#if data.backup.kind == "database" and data.backup.db_type != none [
  #kv("Engine", data.backup.db_type)
]
#if data.backup.kind == "volume" and data.backup.container_id != none [
  #kv("Container ID", mono(data.backup.container_id))
]
#kv("Backup ID", mono(data.backup.id))
#kv("Created", data.backup.created_at)
#kv("Generated", data.generated_at)

// ── Backup integrity ────────────────────────────────────────────────────

#v(1em)
== Backup Integrity

#kv("Filename", mono(data.backup.filename))
#kv("Size", humanbytes(data.backup.size_bytes))
#kv("SHA-256", mono(if data.backup.sha256_hash != none { data.backup.sha256_hash } else { "—" }))
#kv("Previous hash", mono(if data.backup.previous_hash != none { data.backup.previous_hash } else { "—" }))

// ── Verifications ───────────────────────────────────────────────────────

#v(1em)
== Verifications #h(1fr) #text(fill: dp_dim, size: 9pt)[#data.verifications.len() runs]

#if data.verifications.len() == 0 [
  #text(fill: dp_dim)[No verifications recorded for this backup.]
] else [
  #table(
    columns: (auto, auto, auto, auto, 1fr),
    align: (left, left, left, left, left),
    stroke: 0.5pt + dp_dim,
    inset: 5pt,
    table.header(
      text(weight: "bold", size: 9pt)[Status],
      text(weight: "bold", size: 9pt)[Started],
      text(weight: "bold", size: 9pt)[Checks],
      text(weight: "bold", size: 9pt)[Duration],
      text(weight: "bold", size: 9pt)[Notes],
    ),
    ..data.verifications.map(v => (
      status_pill(v.status),
      text(size: 8.5pt)[#if v.started_at != none { v.started_at } else { v.created_at }],
      text(size: 8.5pt)[#v.checks_passed / #v.checks_run],
      text(size: 8.5pt)[#duration_ms(v.duration_ms)],
      text(size: 8.5pt, fill: if v.error_message != none { dp_fail } else { dp_dim })[
        #if v.error_message != none { v.error_message } else { "—" }
      ],
    )).flatten()
  )
]

// ── Drills ──────────────────────────────────────────────────────────────

#v(1em)
== Restore Drills #h(1fr) #text(fill: dp_dim, size: 9pt)[#data.drills.len() runs]

#if data.drills.len() == 0 [
  #text(fill: dp_dim)[No restore drills recorded for this backup.]
] else [
  #table(
    columns: (auto, auto, auto, auto, 1fr),
    align: (left, left, left, left, left),
    stroke: 0.5pt + dp_dim,
    inset: 5pt,
    table.header(
      text(weight: "bold", size: 9pt)[Status],
      text(weight: "bold", size: 9pt)[Started],
      text(weight: "bold", size: 9pt)[HTTP],
      text(weight: "bold", size: 9pt)[Duration],
      text(weight: "bold", size: 9pt)[Body / error],
    ),
    ..data.drills.map(d => (
      status_pill(d.status),
      text(size: 8.5pt)[#if d.started_at != none { d.started_at } else { d.created_at }],
      text(size: 8.5pt)[#if d.http_status != none { str(d.http_status) } else { "—" }],
      text(size: 8.5pt)[#duration_ms(d.duration_ms)],
      text(size: 8.5pt, fill: if d.error_message != none { dp_fail } else { dp_dim })[
        #{
          if d.error_message != none { d.error_message }
          else if d.body_excerpt != none { d.body_excerpt }
          else { "—" }
        }
      ],
    )).flatten()
  )
]

// ── Chain integrity summary ─────────────────────────────────────────────

#v(1em)
== Chain Integrity

#kv("Verifications passed", [#data.chain_integrity.verifications_passed of #data.verifications.len()])
#kv("Drills passed", [#data.chain_integrity.drills_passed of #data.drills.len()])

#v(2em)
#text(size: 8pt, fill: dp_dim)[
  This report is a point-in-time snapshot of one #lower(kind_label) backup
  and its full verification + restore-drill history. Hashes are SHA-256 over
  the backup artifact bytes, recorded when the backup was taken and not
  re-computed since — this document reports what was recorded, and does not
  itself attest that the stored artifact still matches its hash. Restore
  drills end-to-end probe a real restore into a scratch container.
]
