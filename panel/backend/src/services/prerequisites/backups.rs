//! The backups vertical — "will these backups survive losing this machine?"
//!
//! The cheapest check in the layer and the one with the worst failure mode. A
//! backup policy with no destination writes its archives to
//! `/var/backups/dockpanel` on the same disk it is insuring: it protects against
//! a bad deploy or a dropped table, and against nothing else. Disk failure, a
//! deleted VPS, ransomware, or the provider closing the account take the backups
//! with the data.
//!
//! Nothing said so anywhere. The policy form has a Destination dropdown whose
//! empty value is silently accepted, and the run reports `success`.
//!
//! Worth being precise about what changed underneath this check, because it is
//! the reason the check can be trusted: until v2.29.0 the executor **selected
//! `destination_id` and never read it**, so choosing a destination made no
//! difference either. Adding "you have no destination" while ignoring the one
//! people did configure would have been guidance that lies.

use uuid::Uuid;

use super::copy::{
    BACKUPS_DEST_CHECK, BACKUPS_DEST_DELETED, BACKUPS_DEST_LOCAL_NO_OPTIONS,
    BACKUPS_DEST_LOCAL_WITH_OPTIONS, BACKUPS_DEST_UPLOADS, BACKUPS_POLICY_CHECK,
    BACKUPS_POLICY_NONE, BACKUPS_POLICY_NOTHING, BACKUPS_POLICY_RUNNING,
};
use super::{PrereqResult, Remediation};

/// What we know about one policy's off-site story.
#[derive(Debug, Clone)]
pub struct PolicyDestinationFacts {
    pub policy_name: String,
    pub destination_id: Option<Uuid>,
    /// The destination row still exists. A policy can point at a deleted one.
    pub destination_exists: bool,
    pub destination_name: Option<String>,
    /// How many destinations are configured panel-wide — decides whether the fix
    /// is "pick one" or "create one".
    pub destinations_available: usize,
}

/// Check: does this backup policy send its backups anywhere off this server?
///
/// # Severity
///
/// `Warning`, not `Blocking`, and this one is worth defending. Local-only
/// backups are a legitimate choice — plenty of people run a box whose real
/// protection is their provider's snapshots, and refusing to save a policy
/// without a destination would be the panel overriding a decision that is not
/// its to make. What is *not* legitimate is making that choice by accident and
/// never being told.
///
/// The exception is a policy pointing at a destination that no longer exists:
/// still a warning, but a different sentence, because the operator did choose and
/// the choice silently stopped applying.
pub fn check_backup_destination(facts: &PolicyDestinationFacts) -> PrereqResult {
    let name = facts.policy_name.as_str();

    match (facts.destination_id, facts.destination_exists) {
        // Chose one, it's there. The only good outcome.
        (Some(_), true) => BACKUPS_DEST_CHECK.result(
            &BACKUPS_DEST_UPLOADS,
            &[
                ("policy", name),
                (
                    "destination",
                    facts.destination_name.as_deref().unwrap_or("its destination"),
                ),
            ],
        ),

        // Chose one, it's gone. Materially worse than never having chosen: the
        // operator believes this is handled.
        // The fix is on the POLICY, so the link goes where the policy is edited.
        // It used to point at the destinations tab, which lists destinations and
        // cannot attach one — and until this release no screen could, so the
        // advice named an action the product did not offer.
        (Some(_), false) => BACKUPS_DEST_CHECK
            .result(&BACKUPS_DEST_DELETED, &[("policy", name)])
            .with_remediation(Remediation::Link {
                label: "Edit the policy".to_string(),
                href: "/backup-orchestrator?tab=policies".to_string(),
            }),

        // Never chose one. The advice differs by whether there is anything to
        // pick — telling someone to create a destination they already have is
        // how guidance loses its credibility.
        (None, _) => {
            let has_options = facts.destinations_available > 0;
            let available = facts.destinations_available.to_string();
            let rendered = if has_options {
                BACKUPS_DEST_CHECK.result(
                    &BACKUPS_DEST_LOCAL_WITH_OPTIONS,
                    &[("policy", name), ("available", available.as_str())],
                )
            } else {
                BACKUPS_DEST_CHECK.result(&BACKUPS_DEST_LOCAL_NO_OPTIONS, &[("policy", name)])
            };

            // Two different actions, so two different destinations: attaching one
            // you already have happens on the policy, creating one does not.
            rendered.with_remediation(if has_options {
                Remediation::Link {
                    label: "Choose a destination".to_string(),
                    href: "/backup-orchestrator?tab=policies".to_string(),
                }
            } else {
                Remediation::Link {
                    label: "Add a destination".to_string(),
                    href: "/backup-orchestrator?tab=destinations".to_string(),
                }
            })
        }
    }
}

/// Check: is anything being backed up at all?
///
/// Distinct from the destination question and prior to it — a panel with zero
/// enabled policies has nothing to send anywhere.
pub fn check_any_policy_enabled(enabled_policies: usize, sites: usize) -> PrereqResult {
    if sites == 0 {
        return BACKUPS_POLICY_CHECK.result(&BACKUPS_POLICY_NOTHING, &[]);
    }
    if enabled_policies > 0 {
        let count = enabled_policies.to_string();
        return BACKUPS_POLICY_CHECK.result(&BACKUPS_POLICY_RUNNING, &[("count", count.as_str())]);
    }

    let sites_s = sites.to_string();
    BACKUPS_POLICY_CHECK
        .result(&BACKUPS_POLICY_NONE, &[("sites", sites_s.as_str())])
        .with_remediation(Remediation::Link {
            label: "Create a backup policy".to_string(),
            href: "/backup-orchestrator?tab=policies".to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::prerequisites::PrereqState;

    fn facts() -> PolicyDestinationFacts {
        PolicyDestinationFacts {
            policy_name: "Nightly".into(),
            destination_id: None,
            destination_exists: false,
            destination_name: None,
            destinations_available: 0,
        }
    }

    #[test]
    fn a_policy_with_no_destination_warns_and_never_blocks() {
        let r = check_backup_destination(&facts());
        assert_eq!(r.state, PrereqState::Unsatisfied);
        assert!(!r.blocks(), "local-only backups are a legitimate choice");
        assert!(matches!(r.remediation, Some(Remediation::Link { .. })));
    }

    /// The copy has to change, or someone with a destination already configured
    /// is told to go and create one.
    #[test]
    fn the_advice_depends_on_whether_a_destination_exists_to_pick() {
        let mut f = facts();
        f.destinations_available = 2;
        let r = check_backup_destination(&f);
        assert!(r.detail.contains("already have 2"));
        match r.remediation {
            Some(Remediation::Link { ref label, .. }) => assert_eq!(label, "Choose a destination"),
            other => panic!("expected a link, got {other:?}"),
        }
    }

    /// The dangerous state: the operator DID configure off-site backups and the
    /// destination was deleted, so they believe this is handled and it is not.
    #[test]
    fn a_dangling_destination_is_called_out_differently_from_never_choosing_one() {
        let mut f = facts();
        f.destination_id = Some(Uuid::nil());
        f.destination_exists = false;
        let r = check_backup_destination(&f);
        assert_eq!(r.state, PrereqState::Unsatisfied);
        assert!(r.title.contains("no longer exists"));
    }

    #[test]
    fn a_live_destination_is_satisfied_and_names_it() {
        let mut f = facts();
        f.destination_id = Some(Uuid::nil());
        f.destination_exists = true;
        f.destination_name = Some("wasabi-eu".into());
        let r = check_backup_destination(&f);
        assert_eq!(r.state, PrereqState::Satisfied);
        assert!(r.detail.contains("wasabi-eu"));
    }

    #[test]
    fn sites_with_no_policy_warn_but_an_empty_panel_does_not() {
        assert_eq!(check_any_policy_enabled(0, 3).state, PrereqState::Unsatisfied);
        assert_eq!(
            check_any_policy_enabled(0, 0).state,
            PrereqState::Unknown,
            "a panel with no sites is not neglecting anything"
        );
        assert_eq!(check_any_policy_enabled(1, 3).state, PrereqState::Satisfied);
    }

    /// See the equivalent in `apps` — the registry proves its templates *can* be
    /// filled; this proves these callers *do* fill them, which a release build's
    /// compiled-out `debug_assert` cannot.
    #[test]
    fn no_backup_check_leaks_an_unfilled_placeholder() {
        let mut live = facts();
        live.destination_id = Some(Uuid::nil());
        live.destination_exists = true;
        live.destination_name = Some("wasabi-eu".into());

        let mut dangling = facts();
        dangling.destination_id = Some(Uuid::nil());

        let mut with_options = facts();
        with_options.destinations_available = 2;

        // A live destination whose name we somehow don't have — the `unwrap_or`
        // branch, which is the one a placeholder would hide in.
        let mut unnamed = live.clone();
        unnamed.destination_name = None;

        let cases = [
            check_backup_destination(&facts()),
            check_backup_destination(&live),
            check_backup_destination(&dangling),
            check_backup_destination(&with_options),
            check_backup_destination(&unnamed),
            check_any_policy_enabled(0, 0),
            check_any_policy_enabled(0, 3),
            check_any_policy_enabled(2, 3),
        ];

        for r in cases {
            assert!(!r.title.contains('{'), "unfilled placeholder in title: {}", r.title);
            assert!(!r.detail.contains('{'), "unfilled placeholder in detail: {}", r.detail);
        }
    }

    /// Nothing on the backup surface gates a control.
    #[test]
    fn backup_prerequisites_never_gate_a_control() {
        assert!(!check_backup_destination(&facts()).blocks());
        assert!(!check_any_policy_enabled(0, 5).blocks());
    }
}
