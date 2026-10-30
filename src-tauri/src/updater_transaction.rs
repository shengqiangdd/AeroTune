//! Pure transaction coordinator for signed updater artifacts.
//!
//! The state machine performs no filesystem, process, ACL, or installer I/O. A future protected
//! recovery host must persist each returned journal successor before the associated external side
//! effect, then submit independently observed facts here.

#![allow(dead_code)]

#[cfg(test)]
use crate::update_recovery::{TransactionPhase, UpdateTransactionJournal};

// This coordinator is intentionally not called by the current updater runtime. It cannot be
// activated until a separate protected supervisor supplies validated independent system facts.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[cfg(test)]
pub(crate) enum ObservedInstallState {
    TargetVerified,
    Unknown,
    TargetFailedAndQuiescent,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
#[cfg(test)]
pub(crate) enum RecoveryAction {
    PersistInstallIntent,
    WaitForInstallerQuiescence,
    PersistInstallerObserved,
    VerifyTargetManifest,
    VerifyCommittedTarget,
    PersistRollbackIntent,
    RestorePriorArtifact,
    VerifyRolledBackTarget,
    PreserveEvidenceAndBlock,
}

#[derive(Debug, Clone, Eq, PartialEq)]
#[cfg(test)]
pub(crate) struct RecoveryPlan {
    pub actions: Vec<RecoveryAction>,
    pub next_journal: Option<UpdateTransactionJournal>,
}

#[cfg(test)]
fn plan(
    actions: &[RecoveryAction],
    next_journal: Option<UpdateTransactionJournal>,
) -> RecoveryPlan {
    RecoveryPlan {
        actions: actions.to_vec(),
        next_journal,
    }
}

/// Plan a safe recovery step from durable journal state and independently observed facts.
/// Ambiguous states and terminal records remain blocked; no installer is launched by this module.
#[cfg(test)]
pub(crate) fn plan_recovery(
    journal: &UpdateTransactionJournal,
    installer_quiescent: bool,
    install_state: ObservedInstallState,
    prior_artifact_verified: bool,
    rollback_restore_completed: bool,
    target_manifest_verified: bool,
    prior_manifest_verified: bool,
) -> Result<RecoveryPlan, String> {
    journal.validate()?;
    use ObservedInstallState as I;
    use RecoveryAction as A;
    use TransactionPhase as P;
    let blocked = || plan(&[A::PreserveEvidenceAndBlock], None);
    let next = |phase| journal.advance(phase).map(Some);

    Ok(match journal.phase() {
        P::Prepared => {
            if !installer_quiescent
                || install_state != I::TargetFailedAndQuiescent
                || !prior_artifact_verified
            {
                blocked()
            } else {
                plan(&[A::PersistInstallIntent], next(P::InstallIntent)?)
            }
        }
        P::InstallIntent => {
            if !installer_quiescent || install_state == I::Unknown {
                plan(
                    &[A::WaitForInstallerQuiescence, A::PreserveEvidenceAndBlock],
                    None,
                )
            } else if install_state == I::TargetVerified {
                plan(&[A::PersistInstallerObserved], next(P::InstallerObserved)?)
            } else {
                plan(&[A::PersistRollbackIntent], next(P::RollbackRequired)?)
            }
        }
        P::InstallerObserved => plan(&[A::VerifyTargetManifest], None),
        P::Verifying => {
            if target_manifest_verified {
                plan(&[A::VerifyCommittedTarget], next(P::Committed)?)
            } else {
                plan(&[A::PersistRollbackIntent], next(P::RollbackRequired)?)
            }
        }
        P::RollbackRequired => {
            if prior_artifact_verified {
                plan(&[A::PersistRollbackIntent], next(P::RollbackIntent)?)
            } else {
                blocked()
            }
        }
        P::RollbackIntent => {
            if !installer_quiescent {
                plan(
                    &[A::WaitForInstallerQuiescence, A::PreserveEvidenceAndBlock],
                    None,
                )
            } else if rollback_restore_completed {
                plan(&[A::VerifyRolledBackTarget], None)
            } else {
                plan(&[A::RestorePriorArtifact], None)
            }
        }
        P::RollbackObserved => {
            if prior_manifest_verified {
                plan(&[A::VerifyRolledBackTarget], next(P::RolledBack)?)
            } else {
                blocked()
            }
        }
        P::Committed | P::RolledBack => blocked(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update_recovery::TransactionPhase as P;

    fn journal() -> UpdateTransactionJournal {
        UpdateTransactionJournal::test_fixture()
    }

    fn at(phase: P) -> UpdateTransactionJournal {
        let mut current = journal();
        let path: &[P] = match phase {
            P::Prepared => return current,
            P::InstallIntent => &[P::InstallIntent],
            P::InstallerObserved => &[P::InstallIntent, P::InstallerObserved],
            P::Verifying => &[P::InstallIntent, P::InstallerObserved, P::Verifying],
            P::RollbackRequired => &[P::InstallIntent, P::RollbackRequired],
            P::RollbackIntent => &[P::InstallIntent, P::RollbackRequired, P::RollbackIntent],
            P::RollbackObserved => &[
                P::InstallIntent,
                P::RollbackRequired,
                P::RollbackIntent,
                P::RollbackObserved,
            ],
            P::Committed => &[
                P::InstallIntent,
                P::InstallerObserved,
                P::Verifying,
                P::Committed,
            ],
            P::RolledBack => &[
                P::InstallIntent,
                P::RollbackRequired,
                P::RollbackIntent,
                P::RollbackObserved,
                P::RolledBack,
            ],
        };
        for next in path {
            current = current.advance(*next).unwrap();
        }
        current
    }

    #[test]
    fn prepared_requires_durable_intent_and_verified_prior_artifact() {
        let value = journal();
        let blocked = plan_recovery(
            &value,
            true,
            ObservedInstallState::TargetFailedAndQuiescent,
            false,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(blocked.actions, [RecoveryAction::PreserveEvidenceAndBlock]);
        assert!(blocked.next_journal.is_none());
        let ready = plan_recovery(
            &value,
            true,
            ObservedInstallState::TargetFailedAndQuiescent,
            true,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(ready.actions, [RecoveryAction::PersistInstallIntent]);
        assert_eq!(ready.next_journal.unwrap().phase(), P::InstallIntent);
    }

    #[test]
    fn ambiguous_install_never_authorizes_rollback() {
        let intent = at(P::InstallIntent);
        let result = plan_recovery(
            &intent,
            false,
            ObservedInstallState::Unknown,
            false,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            result.actions,
            [
                RecoveryAction::WaitForInstallerQuiescence,
                RecoveryAction::PreserveEvidenceAndBlock
            ]
        );
        assert!(result.next_journal.is_none());
    }

    #[test]
    fn target_manifest_verification_is_a_distinct_phase() {
        let intent = at(P::InstallIntent);
        let observed = plan_recovery(
            &intent,
            true,
            ObservedInstallState::TargetVerified,
            true,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(observed.actions, [RecoveryAction::PersistInstallerObserved]);
        assert_eq!(observed.next_journal.unwrap().phase(), P::InstallerObserved);
        let verifying = at(P::Verifying);
        let commit = plan_recovery(
            &verifying,
            true,
            ObservedInstallState::TargetVerified,
            true,
            false,
            true,
            false,
        )
        .unwrap();
        assert_eq!(commit.actions, [RecoveryAction::VerifyCommittedTarget]);
        assert_eq!(commit.next_journal.unwrap().phase(), P::Committed);
    }

    #[test]
    fn rollback_requires_signed_prior_artifact_and_quiescent_installer() {
        let required = at(P::RollbackRequired);
        let blocked = plan_recovery(
            &required,
            true,
            ObservedInstallState::TargetFailedAndQuiescent,
            false,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(blocked.actions, [RecoveryAction::PreserveEvidenceAndBlock]);
        let intent = plan_recovery(
            &required,
            true,
            ObservedInstallState::TargetFailedAndQuiescent,
            true,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(intent.next_journal.unwrap().phase(), P::RollbackIntent);
        let rollback = at(P::RollbackIntent);
        let waiting = plan_recovery(
            &rollback,
            false,
            ObservedInstallState::Unknown,
            true,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(
            waiting.actions,
            [
                RecoveryAction::WaitForInstallerQuiescence,
                RecoveryAction::PreserveEvidenceAndBlock
            ]
        );
        let restore = plan_recovery(
            &rollback,
            true,
            ObservedInstallState::TargetFailedAndQuiescent,
            true,
            false,
            false,
            false,
        )
        .unwrap();
        assert_eq!(restore.actions, [RecoveryAction::RestorePriorArtifact]);
    }

    #[test]
    fn terminal_journals_are_preserved_until_supervisor_cleanup_exists() {
        for phase in [P::Committed, P::RolledBack] {
            let result = plan_recovery(
                &at(phase),
                true,
                ObservedInstallState::TargetVerified,
                true,
                true,
                true,
                true,
            )
            .unwrap();
            assert_eq!(result.actions, [RecoveryAction::PreserveEvidenceAndBlock]);
            assert!(result.next_journal.is_none());
        }
    }
}
