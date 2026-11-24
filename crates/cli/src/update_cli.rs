//! `commutator update` — check GitHub releases for a newer one and apply it
//! (download → verify sha256 → symlink swap → service restart). macOS app
//! bundles swap the bundle instead; source builds are report-only.

use anyhow::bail;
use commutator_update::{InstallKind, Offer};

/// `--check` prints the verdict and exits (nonzero when an update is available,
/// so scripts can gate on it). A macOS app follows its release channel
/// (`update-channel.json` in the data dir), as the app's own updater does.
pub async fn update(check_only: bool) -> anyhow::Result<()> {
    commutator_update::channel::set_data_dir(&super::paths::data_dir());
    let current = commutator_update::app_version();
    let Some(manifest) = commutator_update::fetch_latest().await? else {
        println!("commutator {current}: no release has been published yet.");
        return Ok(());
    };
    let running_engine = match commutator_update::detect_install() {
        InstallKind::MacApp { bundle } => commutator_update::bundle_engine_version(&bundle),
        _ => None,
    };
    match commutator_update::offer(
        &manifest,
        current,
        manifest.channel,
        running_engine.as_deref(),
        commutator_update::channel::macos_version(),
    ) {
        Offer::Newer | Offer::Switch => {}
        Offer::Held(why) => {
            println!("commutator {current}: {} isn't installable here. {why}", manifest.version);
            return Ok(());
        }
        Offer::Current => {
            println!(
                "commutator {current} is up to date (latest: {}).",
                manifest.version
            );
            return Ok(());
        }
    }
    println!("commutator {current} → {} available", manifest.version);
    if check_only {
        std::process::exit(1);
    }

    let install = commutator_update::detect_install();
    if let Some(blocker) = install.desktop_update_blocker() {
        bail!("{blocker}");
    }
    match install {
        InstallKind::Managed { app_root } if service_running() => {
            println!(
                "downloading {}…",
                commutator_update::headless_artifact(&manifest.version)
            );
            let data_dir = super::paths::data_dir();
            commutator_update::prepare_install(&manifest, &app_root, &data_dir).await?;
            let from = commutator_update::installed_version(&InstallKind::Managed {
                app_root: app_root.clone(),
            })
            .unwrap_or_else(|| commutator_update::current_version().to_owned());
            let requested = commutator_update::transaction::request(
                &data_dir,
                commutator_update::transaction::Layout::Versioned { app_root: app_root.clone() },
                &from,
                &manifest.version,
                commutator_update::transaction::RestartMode::Service {
                    unit: commutator_update::service_unit(),
                },
            )?;
            if let Err(err) = commutator_update::restart_service() {
                commutator_update::transaction::fail_request(
                    &data_dir,
                    &requested.id,
                    "Commutator couldn't restart to install the update, so nothing changed.".into(),
                );
                bail!("couldn't restart the engine service: {err:#}");
            }
            println!("engine service restarting; waiting for {} to prove it's ready…", manifest.version);
            wait_for_outcome(&data_dir, &requested).await
        }
        InstallKind::Managed { app_root } => {
            println!(
                "downloading {}…",
                commutator_update::headless_artifact(&manifest.version)
            );
            commutator_update::stage_headless(&manifest, &app_root).await?;
            commutator_update::apply_headless(&app_root, &manifest.version)?;
            println!(
                "installed {} (current → {})",
                app_root.join(commutator_update::managed_dir(&manifest.version)).display(),
                manifest.version
            );
            match commutator_update::restart_service() {
                Ok(()) => println!("engine service restarted."),
                Err(err) => println!(
                    "note: service restart failed ({err:#}) — restart the engine manually to finish."
                ),
            }
            Ok(())
        }
        InstallKind::MacApp { bundle } => {
            println!(
                "downloading {}…",
                commutator_update::mac_app_artifact(&manifest.version)
            );
            let data_dir = super::paths::data_dir();
            let staged = commutator_update::stage_mac_app(&manifest, &data_dir).await?;
            commutator_update::apply_mac_app(&staged, &bundle)?;
            println!("updated {} — relaunch Commutator to finish.", bundle.display());
            Ok(())
        }
        #[cfg(windows)]
        InstallKind::WindowsPortable { directory } => {
            let staged = commutator_update::windows::stage(&manifest, &directory).await?;
            commutator_update::windows::apply(&staged, &directory, false)?;
            println!(
                "updated to {} — relaunch Commutator to finish.",
                manifest.version
            );
            Ok(())
        }
        InstallKind::Unmanaged => {
            bail!(
                "this binary is not update-managed (source build or hand-copied).\n\
                 Download a release from {}, or rebuild from source.",
                commutator_update::LATEST_RELEASE_PAGE
            )
        }
    }
}

/// Whether the installed service is up: it's what restarts into the update and
/// what can go back. Without one there's nothing running to protect, and the new
/// version simply starts next time.
fn service_running() -> bool {
    let unit = commutator_update::service_unit();
    cfg!(target_os = "linux")
        && commutator_update::systemctl_query(&unit)
            .args(["--quiet", "is-active", &unit])
            .status()
            .is_ok_and(|status| status.success())
}

/// Report how the update went: it may yet go back, so "restarted" says nothing.
async fn wait_for_outcome(
    data_dir: &std::path::Path,
    requested: &commutator_update::transaction::Transaction,
) -> anyhow::Result<()> {
    use commutator_update::UpdateState;
    // The trial's own deadline, plus time to stop and go back.
    let limit = std::time::Duration::from_secs(requested.deadline_secs + 120);
    let started = std::time::Instant::now();
    while started.elapsed() < limit {
        let report = commutator_update::transaction::report(data_dir).filter(|r| r.id == requested.id);
        match report.as_ref().map(|report| report.state) {
            Some(UpdateState::Updated) => {
                println!("updated to {}.", requested.target);
                return Ok(());
            }
            Some(UpdateState::RolledBack | UpdateState::Failed) => {
                let reason = report.and_then(|report| report.reason).unwrap_or_default();
                bail!("the update didn't go through. {reason}");
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
        }
    }
    bail!(
        "the update hasn't finished after {} min. Check `commutator daemon status`.",
        limit.as_secs() / 60
    )
}
