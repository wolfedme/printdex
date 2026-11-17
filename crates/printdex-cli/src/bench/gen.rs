//! Deterministic synthetic model library, shaped like real collections:
//! category folders, model folders with part subfolders, cover images,
//! loose files, mixed formats and casing, non-ASCII names.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Write;
use std::path::Path;

/// What to generate.
#[derive(Clone, Debug)]
pub struct GenSpec {
    pub parts: u64,
    pub seed: u64,
}

/// What was generated.
#[derive(Clone, Debug, Default)]
pub struct GenReport {
    pub models: u64,
    pub parts: u64,
    pub assets: u64,
    pub bytes: u64,
    /// Name vocabulary used, sorted.
    pub words: Vec<String>,
}

/// Category folders models are spread over. None is a prefix of another.
const CATEGORIES: &[&str] = &[
    "Minis/Fantasy",
    "Minis/Sci-Fi",
    "Minis/Monsters",
    "Terrain/Dungeon",
    "Terrain/City/Ruins",
    "Terrain/Nature",
    "Tools",
    "Household/Kitchen",
    "Household/Garden",
    "Cosplay/Props",
    "Cosplay/Armor",
    "Toys",
    "Printer Upgrades",
    "Organizers",
    "Vehicles/Tanks",
    "Vehicles/Spaceships",
    "Jewelry",
    "Busts",
];

/// Model-name vocabulary, including non-ASCII words.
pub const VOCABULARY: &str = "\
    red blue green black white golden silver ancient tiny giant broken hollow \
    dark bright frozen burning silent royal savage wild iron stone wooden crystal \
    rusty mighty little elder young shadow storm fire ice thunder poison holy \
    cursed noble swift heavy light grim lucky arcane feral spiked twin hidden \
    dragon goblin orc knight wizard witch elf dwarf troll giant ogre skeleton \
    zombie vampire wolf bear eagle raven serpent hydra kraken golem demon angel \
    paladin ranger rogue cleric barbarian bard monk druid warlock sorcerer \
    king queen prince princess guard soldier archer captain pirate ninja samurai \
    robot mech drone tank walker trooper marine pilot alien android cyborg \
    tower castle bridge gate wall ruin temple shrine altar statue fountain well \
    tree rock cliff cave crate barrel chest table chair door window ladder \
    hook clip bracket holder stand mount box tray lid hinge knob handle cover \
    gear spool fan duct nozzle clamp vase planter pot cup bowl coaster sign \
    sword axe shield helmet mask bow staff wand dagger spear hammer crown ring \
    ship rover buggy cannon turret engine wing rocket station hangar base \
    bust head torso arm leg cape cloak banner flag lantern torch candle book \
    Drachen Château Ü-Boot 龍 Schloß Ritter Hexe Zwerg café niño señor fée \
    Æther Øre Ölfass Bär Wolfsmond dragón ドラゴン 城 剣 мечь рыцарь";

const PART_FOLDERS: &[&str] = &["STLs", "supported", "unsupported"];

/// Writes a synthetic library under `root`.
pub fn generate(root: &Path, spec: &GenSpec) -> std::io::Result<GenReport> {
    let vocab: Vec<&str> = VOCABULARY.split_whitespace().collect();
    let mut rng = fastrand::Rng::with_seed(spec.seed);
    let mut report = GenReport::default();
    let mut words_used = BTreeSet::new();
    let mut taken: HashSet<String> = HashSet::new();
    let mut left = spec.parts;
    let mut loose = 0u64;

    fs::create_dir_all(root)?;
    while left > 0 {
        let category = CATEGORIES[rng.usize(..CATEGORIES.len())];
        let n_words = rng.usize(1..=3);
        let words: Vec<&str> = (0..n_words)
            .map(|_| vocab[rng.usize(..vocab.len())])
            .collect();
        words_used.extend(words.iter().map(|w| w.to_string()));

        // About 5 % of parts are loose files directly in a category folder.
        if rng.f32() < 0.25 {
            loose += 1;
            let name = format!("{} {loose}", words.join("_"));
            let ext = extension(&mut rng);
            let rel = format!("{category}/{name}.{ext}");
            report.bytes += write_part(root, &rel, &ext, &mut rng)?;
            report.parts += 1;
            report.models += 1;
            left -= 1;
            continue;
        }

        let mut name = words.join(" ");
        let mut n = 1;
        // Case-insensitive, so Windows and macOS file systems never merge two models.
        while !taken.insert(format!("{category}/{name}").to_lowercase()) {
            n += 1;
            name = format!("{} {n}", words.join(" "));
        }
        let model_dir = format!("{category}/{name}");
        let parts = rng.u64(1..=12).min(left);
        let part_folder = (rng.f32() < 0.4).then(|| PART_FOLDERS[rng.usize(..PART_FOLDERS.len())]);
        for k in 0..parts {
            let ext = extension(&mut rng);
            let rel = match part_folder {
                Some(f) => format!("{model_dir}/{f}/part_{k}.{ext}"),
                None => format!("{model_dir}/part_{k}.{ext}"),
            };
            report.bytes += write_part(root, &rel, &ext, &mut rng)?;
        }
        if rng.bool() {
            let cover: Vec<u8> = (0..rng.usize(24..64)).map(|_| rng.u8(..)).collect();
            report.bytes += write(root, &format!("{model_dir}/cover.jpg"), &cover)?;
            let readme = format!("# {name}\n\nGenerated by printdex bench.\n");
            report.bytes += write(root, &format!("{model_dir}/README.md"), readme.as_bytes())?;
            report.assets += 2;
        }
        report.models += 1;
        report.parts += parts;
        left -= parts;
    }

    report.words = words_used.into_iter().collect();
    Ok(report)
}

/// 85 % STL, 10 % 3MF, 5 % OBJ; one in ten upper-case.
fn extension(rng: &mut fastrand::Rng) -> String {
    let r = rng.f32();
    let ext = if r < 0.85 {
        "stl"
    } else if r < 0.95 {
        "3mf"
    } else {
        "obj"
    };
    if rng.f32() < 0.1 {
        ext.to_ascii_uppercase()
    } else {
        ext.to_string()
    }
}

fn write_part(root: &Path, rel: &str, ext: &str, rng: &mut fastrand::Rng) -> std::io::Result<u64> {
    match ext.to_ascii_lowercase().as_str() {
        "stl" => write(root, rel, &binary_stl(rng)),
        // An empty zip archive (end-of-central-directory record only).
        "3mf" => write(root, rel, b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0"),
        _ => write(root, rel, b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"),
    }
}

/// A valid binary STL with 1–200 random triangles.
fn binary_stl(rng: &mut fastrand::Rng) -> Vec<u8> {
    let triangles = rng.u32(1..=200);
    let mut out = Vec::with_capacity(84 + 50 * triangles as usize);
    let mut header = [b' '; 80];
    header[..14].copy_from_slice(b"printdex bench");
    out.extend_from_slice(&header);
    out.extend_from_slice(&triangles.to_le_bytes());
    for _ in 0..triangles {
        // normal + 3 vertices
        for _ in 0..12 {
            out.extend_from_slice(&(rng.f32() * 100.0).to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
    }
    out
}

fn write(root: &Path, rel: &str, bytes: &[u8]) -> std::io::Result<u64> {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::File::create(&path)?.write_all(bytes)?;
    Ok(bytes.len() as u64)
}
