//! Runs the built `printdex` binary end to end.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn printdex(data: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_printdex"))
        .arg("--data-dir")
        .arg(data)
        .args(args)
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, rel.as_bytes()).unwrap();
}

fn library(dir: &Path) -> String {
    let root = dir.join("lib");
    touch(&root, "Red Dragon/a.stl");
    root.to_str().unwrap().to_owned()
}

#[test]
fn scan_then_search_prints_hit() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    let lib = library(d.path());
    let o = printdex(&data, &["scan", &lib]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("models: +1"), "{}", stdout(&o));
    let o = printdex(&data, &["search", "dragon"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        stdout(&o).contains("Red Dragon\tRed Dragon\t1 parts"),
        "{}",
        stdout(&o)
    );
    let o = printdex(&data, &["search", "nothing-like-this"]);
    assert!(o.status.success());
    assert!(stdout(&o).is_empty());
}

#[test]
fn second_scan_fingerprints_nothing() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    let lib = library(d.path());
    assert!(printdex(&data, &["scan", &lib]).status.success());
    let o = printdex(&data, &["scan", &lib]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("fingerprinted 0"), "{}", stdout(&o));
}

#[test]
fn scan_missing_path_fails() {
    let d = tempfile::tempdir().unwrap();
    let missing = d.path().join("nope");
    let o = printdex(&d.path().join("data"), &["scan", missing.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).starts_with("error:"), "{}", stderr(&o));
}

#[test]
fn stats_prints_counts() {
    let d = tempfile::tempdir().unwrap();
    let data = d.path().join("data");
    let lib = library(d.path());
    assert!(printdex(&data, &["scan", &lib]).status.success());
    let o = printdex(&data, &["stats"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("models: 1"), "{out}");
    assert!(out.contains("parts: 1"), "{out}");
}
