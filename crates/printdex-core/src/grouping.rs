//! Groups scanned files into models (v1 design §6).
//!
//! A model is either a folder (with all printable parts and assets in its
//! subtree) or a single printable file. The rule:
//!
//! ```text
//! "printable dir" = a directory that directly contains ≥1 printable file
//! "has printables" = its subtree contains ≥1 printable file
//! "part folder"   = a directory whose name is in PART_FOLDER_NAMES
//!
//! visit(dir, is_library_root):
//!   P = printable files directly in dir
//!   C = child dirs that have printables
//!   if is_library_root:
//!       each file in P → SingleFile model; visit each child in C
//!   else if P non-empty:
//!       if C contains any non-part folder:            # container with loose files
//!           each file in P → SingleFile model; visit each child in C
//!       else:
//!           Folder model rooted at dir (whole subtree)
//!   else (P empty):
//!       if C non-empty and every child in C is a part folder:
//!           Folder model rooted at dir (whole subtree)
//!       else:
//!           visit each child in C
//! ```
//!
//! Non-printable files outside any folder model are ignored.

use std::collections::BTreeMap;

use crate::walk::{FileEntry, FileKind};

/// How a model maps onto the file system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelKind {
    Folder,
    SingleFile,
}

impl ModelKind {
    /// Stable name, as stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelKind::Folder => "folder",
            ModelKind::SingleFile => "file",
        }
    }

    /// Parses the stored name.
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "folder" => Some(ModelKind::Folder),
            "file" => Some(ModelKind::SingleFile),
            _ => None,
        }
    }
}

/// One model found in a scan. `parts` and `assets` index into the input
/// slice, ascending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelGroup {
    pub rel_path: String,
    pub name: String,
    pub kind: ModelKind,
    pub parts: Vec<usize>,
    pub assets: Vec<usize>,
}

/// Folder names that hold a model's parts rather than separate models
/// (matched ignoring ASCII case).
pub const PART_FOLDER_NAMES: &[&str] = &[
    "stl",
    "stls",
    "3mf",
    "obj",
    "files",
    "parts",
    "print files",
    "printfiles",
    "supported",
    "unsupported",
    "presupported",
    "pre-supported",
    "pre supported",
    "lychee",
    "chitubox",
];

/// Groups `files` into models, sorted by `rel_path`. Pure, no I/O.
pub fn group_models(files: &[FileEntry]) -> Vec<ModelGroup> {
    let mut root = DirNode::default();
    for (i, f) in files.iter().enumerate() {
        let (dirs, _file) = match f.rel_path.rsplit_once('/') {
            Some((dirs, file)) => (Some(dirs), file),
            None => (None, f.rel_path.as_str()),
        };
        let mut node = &mut root;
        let printable = matches!(f.kind, FileKind::Printable(_));
        node.has_printables |= printable;
        for seg in dirs.into_iter().flat_map(|d| d.split('/')) {
            node = node.children.entry(seg).or_default();
            node.has_printables |= printable;
        }
        if printable {
            node.printables.push(i);
        } else {
            node.others.push(i);
        }
    }

    let mut out = Vec::new();
    visit(&root, "", true, files, &mut out);
    out.sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

#[derive(Default)]
struct DirNode<'a> {
    children: BTreeMap<&'a str, DirNode<'a>>,
    /// Printable files directly in this directory.
    printables: Vec<usize>,
    /// Other files directly in this directory.
    others: Vec<usize>,
    /// Whether the subtree holds at least one printable file.
    has_printables: bool,
}

fn is_part_folder(name: &str) -> bool {
    PART_FOLDER_NAMES
        .iter()
        .any(|p| p.eq_ignore_ascii_case(name))
}

fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

fn visit(
    node: &DirNode<'_>,
    path: &str,
    is_root: bool,
    files: &[FileEntry],
    out: &mut Vec<ModelGroup>,
) {
    let mut children = node
        .children
        .iter()
        .filter(|(_, c)| c.has_printables)
        .peekable();
    let has_children = children.peek().is_some();
    let all_part_folders = node
        .children
        .iter()
        .filter(|(_, c)| c.has_printables)
        .all(|(name, _)| is_part_folder(name));

    let folder_model = !is_root
        && if node.printables.is_empty() {
            has_children && all_part_folders
        } else {
            all_part_folders
        };

    if folder_model {
        let mut parts = Vec::new();
        let mut assets = Vec::new();
        collect(node, &mut parts, &mut assets);
        parts.sort_unstable();
        assets.sort_unstable();
        out.push(ModelGroup {
            rel_path: path.to_string(),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            kind: ModelKind::Folder,
            parts,
            assets,
        });
        return;
    }

    for &i in &node.printables {
        let rel_path = &files[i].rel_path;
        let file_name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        let name = file_name
            .rsplit_once('.')
            .map_or(file_name, |(stem, _)| stem);
        out.push(ModelGroup {
            rel_path: rel_path.clone(),
            name: name.to_string(),
            kind: ModelKind::SingleFile,
            parts: vec![i],
            assets: Vec::new(),
        });
    }
    for (name, child) in children {
        visit(child, &join(path, name), false, files, out);
    }
}

fn collect(node: &DirNode<'_>, parts: &mut Vec<usize>, assets: &mut Vec<usize>) {
    parts.extend(&node.printables);
    assets.extend(&node.others);
    for child in node.children.values() {
        collect(child, parts, assets);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walk::classify;

    fn entries(paths: &[&str]) -> Vec<FileEntry> {
        let mut v: Vec<FileEntry> = paths
            .iter()
            .map(|p| FileEntry {
                rel_path: (*p).to_string(),
                size: 1,
                mtime_ns: 1,
                kind: classify(p.rsplit('/').next().unwrap()),
            })
            .collect();
        v.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
        v
    }

    fn summary(groups: &[ModelGroup]) -> Vec<(&str, &str, ModelKind, usize, usize)> {
        groups
            .iter()
            .map(|g| {
                (
                    g.rel_path.as_str(),
                    g.name.as_str(),
                    g.kind,
                    g.parts.len(),
                    g.assets.len(),
                )
            })
            .collect()
    }

    use ModelKind::{Folder, SingleFile};

    #[test]
    fn folder_with_files_is_one_model() {
        let f = entries(&["Dragon/a.stl", "Dragon/b.3mf", "Dragon/cover.jpg"]);
        assert_eq!(
            summary(&group_models(&f)),
            [("Dragon", "Dragon", Folder, 2, 1)]
        );
    }

    #[test]
    fn part_subfolders_merge_into_parent() {
        let f = entries(&[
            "Dragon/STLs/a.stl",
            "Dragon/Supported/a_sup.stl",
            "Dragon/readme.txt",
        ]);
        assert_eq!(
            summary(&group_models(&f)),
            [("Dragon", "Dragon", Folder, 2, 1)]
        );
    }

    #[test]
    fn printable_dir_absorbs_any_subfolders_when_all_are_part_folders() {
        let f = entries(&["Dragon/body.stl", "Dragon/supported/body.stl"]);
        assert_eq!(
            summary(&group_models(&f)),
            [("Dragon", "Dragon", Folder, 2, 0)]
        );
    }

    #[test]
    fn category_folders_are_not_models() {
        let f = entries(&["Minis/Dragons/Red/a.stl", "Minis/Dragons/Blue/b.stl"]);
        assert_eq!(
            summary(&group_models(&f)),
            [
                ("Minis/Dragons/Blue", "Blue", Folder, 1, 0),
                ("Minis/Dragons/Red", "Red", Folder, 1, 0),
            ]
        );
    }

    #[test]
    fn loose_file_next_to_model_folders_is_single_file_model() {
        let f = entries(&["Minis/goblin.stl", "Minis/Orc/orc.stl"]);
        assert_eq!(
            summary(&group_models(&f)),
            [
                ("Minis/Orc", "Orc", Folder, 1, 0),
                ("Minis/goblin.stl", "goblin", SingleFile, 1, 0),
            ]
        );
    }

    #[test]
    fn files_in_library_root_are_single_file_models() {
        let f = entries(&["a.stl", "Orc/orc.stl"]);
        assert_eq!(
            summary(&group_models(&f)),
            [
                ("Orc", "Orc", Folder, 1, 0),
                ("a.stl", "a", SingleFile, 1, 0)
            ]
        );
    }

    #[test]
    fn folder_of_only_part_folders_without_name_match() {
        let f = entries(&["Kit/Base/x.stl", "Kit/Arm/y.stl"]);
        assert_eq!(
            summary(&group_models(&f)),
            [
                ("Kit/Arm", "Arm", Folder, 1, 0),
                ("Kit/Base", "Base", Folder, 1, 0)
            ]
        );
    }

    #[test]
    fn assets_outside_models_are_ignored() {
        let f = entries(&["Minis/index.pdf", "Minis/Orc/orc.stl"]);
        assert_eq!(
            summary(&group_models(&f)),
            [("Minis/Orc", "Orc", Folder, 1, 0)]
        );
    }

    #[test]
    fn part_folder_match_is_case_insensitive() {
        let f = entries(&["X/STLS/a.stl", "Y/Pre-Supported/b.stl"]);
        assert_eq!(
            summary(&group_models(&f)),
            [("X", "X", Folder, 1, 0), ("Y", "Y", Folder, 1, 0)]
        );
    }

    #[test]
    fn indices_point_into_input() {
        let f = entries(&["Dragon/a.stl", "Dragon/cover.jpg", "Dragon/z.stl"]);
        let g = group_models(&f);
        assert_eq!(g[0].parts, [0, 2]);
        assert_eq!(g[0].assets, [1]);
    }
}
