//! Backup repository wrapper over rustic, on a folder target.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use printdex_core::BackupError;
use printdex_core::CoreError;
use printdex_core::backup::{
    BackupJob, Globs, Repo, RepoOptions, SnapshotFilter, Target, TargetSecrets,
};
use tokio_util::sync::CancellationToken;

fn folder(dir: &Path) -> Target {
    Target::Folder {
        path: dir.to_path_buf(),
    }
}

fn secrets(pass: &str) -> TargetSecrets {
    TargetSecrets {
        secret: None,
        passphrase: pass.to_owned(),
    }
}

/// A source tree with a text STL (compresses well) and a random binary.
fn source(dir: &Path) {
    std::fs::create_dir_all(dir.join("Hook/parts")).unwrap();
    let mut stl = String::from("solid hook\n");
    for i in 0..4000 {
        stl.push_str(&format!(
            "facet normal 0 0 1\n outer loop\n  vertex {i} 0 0\n  vertex 0 {i} 0\n  vertex 0 0 {i}\n endloop\nendfacet\n"
        ));
    }
    stl.push_str("endsolid hook\n");
    std::fs::write(dir.join("Hook/hook.stl"), stl).unwrap();
    let mut rng = fastrand::Rng::with_seed(7);
    let mut noise = vec![0u8; 300_000];
    rng.fill(&mut noise);
    std::fs::write(dir.join("Hook/parts/noise.bin"), noise).unwrap();
}

fn backup(
    repo: &mut Repo,
    src: &Path,
) -> printdex_core::Result<printdex_core::backup::SnapshotInfo> {
    repo.backup(
        &BackupJob {
            source: src,
            as_path: "/library",
            filter: &Globs::default(),
            tags: &["printdex-models:test".to_owned()],
            host: "host-a",
        },
        &CancellationToken::new(),
        None,
    )
}

fn round_trip(pass: &str) {
    let tmp = tempfile::tempdir().unwrap();
    let (src, target, out) = (
        tmp.path().join("src"),
        tmp.path().join("repo"),
        tmp.path().join("out"),
    );
    source(&src);
    let mut repo =
        Repo::open_or_init(&folder(&target), &secrets(pass), &RepoOptions::default()).unwrap();
    let snap = backup(&mut repo, &src).unwrap();
    assert_eq!(snap.host, "host-a");
    assert_eq!(snap.tags, ["printdex-models:test"]);
    assert_eq!(snap.files, 2);
    drop(repo);

    // Reopen and restore everything, then one subfolder.
    let mut repo = Repo::open(&folder(&target), &secrets(pass), &RepoOptions::default()).unwrap();
    let snaps = repo.snapshots(&SnapshotFilter::default()).unwrap();
    assert_eq!(snaps.len(), 1);
    repo.restore(&snaps[0].id, "", &out, &CancellationToken::new(), None)
        .unwrap();
    for f in ["Hook/hook.stl", "Hook/parts/noise.bin"] {
        assert_eq!(
            std::fs::read(src.join(f)).unwrap(),
            std::fs::read(out.join(f)).unwrap(),
            "{f}"
        );
    }
    let part = tmp.path().join("part");
    repo.restore(
        &snaps[0].id,
        "Hook/parts",
        &part,
        &CancellationToken::new(),
        None,
    )
    .unwrap();
    assert!(part.join("noise.bin").is_file());
    assert!(!part.join("hook.stl").exists());
}

#[test]
fn empty_passphrase_round_trip() {
    round_trip("");
}

#[test]
fn encrypted_round_trip() {
    round_trip("correct horse battery staple");
}

#[test]
fn wrong_passphrase_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let target = folder(&tmp.path().join("repo"));
    Repo::open_or_init(&target, &secrets("right"), &RepoOptions::default()).unwrap();
    let err = Repo::open_or_init(&target, &secrets("wrong"), &RepoOptions::default()).unwrap_err();
    assert!(
        matches!(err, CoreError::Backup(BackupError::WrongPassphrase)),
        "{err:?}"
    );
    // Opening for restore with no repository there says so.
    let empty = folder(&tmp.path().join("nothing"));
    let err = Repo::open(&empty, &secrets(""), &RepoOptions::default()).unwrap_err();
    assert!(
        matches!(err, CoreError::Backup(BackupError::NoRepository)),
        "{err:?}"
    );
}

#[test]
fn dedup_stores_identical_file_once() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, target) = (tmp.path().join("src"), tmp.path().join("repo"));
    source(&src);
    let mut repo =
        Repo::open_or_init(&folder(&target), &secrets(""), &RepoOptions::default()).unwrap();
    backup(&mut repo, &src).unwrap();
    let one = repo.size().unwrap();
    // A second copy of the same model.
    std::fs::create_dir_all(src.join("Copy/parts")).unwrap();
    std::fs::copy(src.join("Hook/hook.stl"), src.join("Copy/hook.stl")).unwrap();
    std::fs::copy(
        src.join("Hook/parts/noise.bin"),
        src.join("Copy/parts/noise.bin"),
    )
    .unwrap();
    let snap = backup(&mut repo, &src).unwrap();
    assert_eq!(snap.files, 4);
    let two = repo.size().unwrap();
    assert!(
        (two as f64) < one as f64 * 1.1,
        "repository grew from {one} to {two}"
    );
}

#[test]
fn zstd_compression_on() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, target) = (tmp.path().join("src"), tmp.path().join("repo"));
    std::fs::create_dir_all(&src).unwrap();
    // Only the text STL, which compresses to a fraction.
    source(&tmp.path().join("full"));
    std::fs::copy(tmp.path().join("full/Hook/hook.stl"), src.join("hook.stl")).unwrap();
    let stl = std::fs::metadata(src.join("hook.stl")).unwrap().len();
    let mut repo =
        Repo::open_or_init(&folder(&target), &secrets(""), &RepoOptions::default()).unwrap();
    backup(&mut repo, &src).unwrap();
    let size = repo.size().unwrap();
    assert!(size < stl / 3, "{stl} bytes stored as {size}");
}

#[test]
fn cancel_mid_backup_leaves_repo_valid() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, target) = (tmp.path().join("src"), tmp.path().join("repo"));
    std::fs::create_dir_all(&src).unwrap();
    // Enough incompressible data for several packs.
    let mut rng = fastrand::Rng::with_seed(1);
    for i in 0..12 {
        let mut data = vec![0u8; 2_000_000];
        rng.fill(&mut data);
        std::fs::write(src.join(format!("big{i}.bin")), data).unwrap();
    }
    let mut repo =
        Repo::open_or_init(&folder(&target), &secrets(""), &RepoOptions::default()).unwrap();
    let cancel = CancellationToken::new();
    let seen = Arc::new(AtomicU64::new(0));
    let progress = {
        let (cancel, seen) = (cancel.clone(), seen.clone());
        Arc::new(move |done: u64, _total: u64| {
            seen.store(done, Ordering::Relaxed);
            if done > 4_000_000 {
                cancel.cancel();
            }
        })
    };
    let err = repo
        .backup(
            &BackupJob {
                source: &src,
                as_path: "/library",
                filter: &Globs::default(),
                tags: &[],
                host: "host-a",
            },
            &cancel,
            Some(progress),
        )
        .unwrap_err();
    assert!(matches!(err, CoreError::Cancelled), "{err:?}");
    assert!(seen.load(Ordering::Relaxed) > 0, "progress was reported");
    drop(repo);

    // The repository still opens and checks clean, has no snapshot, and the
    // next backup completes.
    let mut repo =
        Repo::open_or_init(&folder(&target), &secrets(""), &RepoOptions::default()).unwrap();
    assert!(
        repo.snapshots(&SnapshotFilter::default())
            .unwrap()
            .is_empty()
    );
    repo.check(100.0).unwrap();
    let snap = repo
        .backup(
            &BackupJob {
                source: &src,
                as_path: "/library",
                filter: &Globs::default(),
                tags: &[],
                host: "host-a",
            },
            &CancellationToken::new(),
            None,
        )
        .unwrap();
    assert_eq!(snap.files, 12);
    repo.check(100.0).unwrap();
}

#[test]
fn forget_keeps_other_hosts_and_tags() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, target) = (tmp.path().join("src"), tmp.path().join("repo"));
    source(&src);
    let mut repo =
        Repo::open_or_init(&folder(&target), &secrets(""), &RepoOptions::default()).unwrap();
    let run = |repo: &mut Repo, host: &str, tag: &str| {
        repo.backup(
            &BackupJob {
                source: &src,
                as_path: "/library",
                filter: &Globs::default(),
                tags: &[tag.to_owned()],
                host,
            },
            &CancellationToken::new(),
            None,
        )
        .unwrap();
    };
    for _ in 0..3 {
        run(&mut repo, "host-a", "printdex-data");
        run(&mut repo, "host-b", "printdex-data");
    }
    run(&mut repo, "host-a", "printdex-models:x");
    // All from today: keeping one daily leaves one of host-a's data snapshots.
    let keep = printdex_core::backup::Keep {
        daily: 1,
        weekly: 0,
        monthly: 0,
    };
    assert_eq!(repo.forget(keep, "host-a", "printdex-data").unwrap(), 2);
    let count = |host: &str, tag: &str| {
        repo.snapshots(&SnapshotFilter {
            host: Some(host.into()),
            tag: Some(tag.into()),
        })
        .unwrap()
        .len()
    };
    assert_eq!(count("host-a", "printdex-data"), 1);
    assert_eq!(count("host-b", "printdex-data"), 3);
    assert_eq!(count("host-a", "printdex-models:x"), 1);
    repo.prune().unwrap();
    repo.check(100.0).unwrap();
}

#[test]
fn excludes_skip_matching_files() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, target, out) = (
        tmp.path().join("src"),
        tmp.path().join("repo"),
        tmp.path().join("out"),
    );
    source(&src);
    std::fs::write(src.join("Hook/.DS_Store"), b"x").unwrap();
    let mut repo =
        Repo::open_or_init(&folder(&target), &secrets(""), &RepoOptions::default()).unwrap();
    let snap = repo
        .backup(
            &BackupJob {
                source: &src,
                as_path: "/library",
                filter: &Globs::excluding(&[".DS_Store", "parts/"]),
                tags: &[],
                host: "h",
            },
            &CancellationToken::new(),
            None,
        )
        .unwrap();
    repo.restore(&snap.id, "", &out, &CancellationToken::new(), None)
        .unwrap();
    assert!(out.join("Hook/hook.stl").is_file());
    assert!(!out.join("Hook/.DS_Store").exists());
    assert!(!out.join("Hook/parts").exists());
}

#[test]
fn passphrase_change_keeps_snapshots() {
    let tmp = tempfile::tempdir().unwrap();
    let (src, target) = (tmp.path().join("src"), tmp.path().join("repo"));
    source(&src);
    let mut repo =
        Repo::open_or_init(&folder(&target), &secrets("old"), &RepoOptions::default()).unwrap();
    backup(&mut repo, &src).unwrap();
    repo.change_passphrase("new").unwrap();
    drop(repo);
    assert!(Repo::open(&folder(&target), &secrets("old"), &RepoOptions::default()).is_err());
    let repo = Repo::open(&folder(&target), &secrets("new"), &RepoOptions::default()).unwrap();
    assert_eq!(repo.snapshots(&SnapshotFilter::default()).unwrap().len(), 1);
}
