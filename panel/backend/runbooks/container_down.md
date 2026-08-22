# Container stopped

A managed Docker container is no longer running. Unlike `container_crashloop`, this fires once and indicates a clean stop — either operator-initiated or the container exited and restart policy is `no`.

## Why this fired

The container moved from `running` to `exited`/`dead` and stayed that way, **and the panel has no record of anyone asking for it**.

Since v2.144.0 that second half is the point. A stop the panel carried out — the Stop button, Sleep, a stack stop, or the auto-sleep policy — is recorded and does **not** raise this alert. So if you are reading this, the container went down on its own, or it was stopped from outside the panel (a shell on the box, `docker compose down`, a systemd unit).

This is informational by severity — it does not open an incident on your public status page. If the stop was unplanned, the customer-facing signal will usually be a 5xx or a `service_down` alert on whatever depended on this container.

## First check

1. From the panel: **Apps** — a container the panel stopped on purpose is listed under *"stopped on purpose — not alerting"*, with the reason. This container will **not** be in that list, which is why it alerted.
2. From a terminal:
   ```
   docker ps -a --format '{{.Names}}\t{{.Status}}\t{{.Image}}' | grep -i exited
   docker inspect <container> --format '{{.State.ExitCode}} {{.State.FinishedAt}}'
   docker logs --tail 100 <container>
   ```

## Common causes

- Operator stopped it intentionally (release, migration, debugging)
- Restart policy is `no` and the process exited cleanly
- `docker compose down` ran but only some services were brought back
- Image was pulled and the old container wasn't replaced
- Stopped from outside the panel — a shell on the box, `docker compose down`, an orchestrator
- Auto-sleep policy stopped an idle container (this is correct behavior, and since v2.144.0 it no longer reaches this alert)

## Escalation

Don't page on this alone. Investigate only if a customer reports a problem or if a paired alert fires (`service_down`, dependent containers becoming unhealthy). If the stop was unintentional, restart from the panel and review whether a restart policy should be added.
