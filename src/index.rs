use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rayon::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    Cpp,
    C,
    Header,
    Rust,
    Python,
    Java,
    Js,
    Ts,
    Go,
    Css,
    Html,
    Json,
    Markdown,
    Build,
    Text,
    Binary,
    Other,
}

impl Lang {
    pub fn from_path(path: &Path) -> Lang {
        match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
            Some("cc" | "cpp" | "cxx" | "c++" | "mm" | "m") => Lang::Cpp,
            Some("c") => Lang::C,
            Some("h" | "hh" | "hpp" | "hxx" | "h++") => Lang::Header,
            Some("rs") => Lang::Rust,
            Some("py") => Lang::Python,
            Some("java" | "kt" | "kts") => Lang::Java,
            Some("js" | "mjs" | "cjs" | "jsx") => Lang::Js,
            Some("ts" | "tsx") => Lang::Ts,
            Some("go") => Lang::Go,
            Some("css" | "scss" | "less") => Lang::Css,
            Some("html" | "htm" | "xml" | "svg") => Lang::Html,
            Some("json" | "json5" | "yaml" | "yml" | "toml" | "ini") => Lang::Json,
            Some("md" | "rst" | "txt") => Lang::Markdown,
            Some("gn" | "gni" | "bp" | "mk" | "cmake" | "bzl" | "sh" | "bat" | "ps1") => Lang::Build,
            Some("png" | "jpg" | "jpeg" | "gif" | "ico" | "webp" | "avif" | "bmp" | "icns" | "psd"
            | "woff" | "woff2" | "ttf" | "otf" | "eot" | "zip" | "gz" | "bz2" | "xz" | "zst" | "7z"
            | "rar" | "tar" | "pdf" | "wasm" | "exe" | "dll" | "so" | "dylib" | "a" | "o" | "obj"
            | "lib" | "bin" | "dat" | "db" | "sqlite" | "pack" | "crx" | "jar" | "apk" | "mp4"
            | "mp3" | "mov" | "webm" | "wav" | "flac" | "ogg" | "pyc" | "pyo" | "class" | "dex"
            | "pak" | "idx" | "dic" | "raw" | "pb" | "tflite" | "onnx" | "pbmm") => Lang::Binary,
            None => Lang::Other,
            Some(_) => Lang::Other,
        }
    }

    pub fn is_binary(self) -> bool {
        self == Lang::Binary
    }

    pub fn color(self) -> [f32; 3] {
        let (h, s, l) = match self {
            Lang::Cpp => (212.0, 0.58, 0.60),
            Lang::C => (196.0, 0.52, 0.52),
            Lang::Header => (258.0, 0.45, 0.64),
            Lang::Rust => (20.0, 0.72, 0.56),
            Lang::Python => (46.0, 0.68, 0.56),
            Lang::Java => (2.0, 0.62, 0.58),
            Lang::Js => (52.0, 0.78, 0.60),
            Lang::Ts => (214.0, 0.72, 0.58),
            Lang::Go => (188.0, 0.66, 0.50),
            Lang::Css => (284.0, 0.52, 0.62),
            Lang::Html => (14.0, 0.72, 0.58),
            Lang::Json => (92.0, 0.32, 0.50),
            Lang::Markdown => (222.0, 0.14, 0.56),
            Lang::Build => (142.0, 0.38, 0.46),
            Lang::Text => (220.0, 0.10, 0.52),
            Lang::Binary => (240.0, 0.05, 0.38),
            Lang::Other => (235.0, 0.22, 0.52),
        };
        hsl(h, s, l)
    }
}

pub fn hsl(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match h as u32 / 60 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r + m, g + m, b + m]
}

#[derive(Clone, Copy)]
pub struct Node {
    pub name_start: u32,
    pub name_len: u32,
    pub is_dir: bool,
    pub lang: Lang,
    pub depth: u16,
    pub lines: u32,
    pub weight: f64,
    pub rect: [f32; 4],
    pub children_start: u32,
    pub children_count: u32,
    pub path_start: u32,
    pub path_len: u32,
}

pub struct Scene {
    pub nodes: Vec<Node>,
    pub children_ids: Vec<u32>,
    pub names: Vec<u8>,
    pub paths: Vec<u8>,
    pub root: usize,
    pub stats: Stats,
}

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub files: u64,
    pub dirs: u64,
    pub lines: u64,
    pub bytes: u64,
}

#[derive(Default)]
pub struct Progress {
    pub files_walked: AtomicU64,
    pub files_counted: AtomicU64,
    pub lines: AtomicU64,
    pub bytes: AtomicU64,
    pub phase: AtomicU64,
    pub total_files: AtomicU64,
}

struct BNode {
    name: String,
    path: PathBuf,
    is_dir: bool,
    lang: Lang,
    lines: u32,
    bytes: u64,
    weight: f64,
    children: Vec<u32>,
}

const MAX_COUNT_SIZE: u64 = 12 * 1024 * 1024;

pub fn build(root: &Path, progress: &Arc<Progress>) -> Scene {
    progress.phase.store(1, Ordering::Relaxed);
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

    let walker = jwalk::WalkDir::new(&root)
        .sort(true)
        .skip_hidden(false)
        .process_read_dir(|_depth, _parent, _state, children| {
            children.retain(|e| match e {
                Ok(entry) => {
                    if !entry.file_type.is_dir() {
                        return true;
                    }
                    let n = entry.file_name.to_string_lossy();
                    !matches!(
                        n.as_ref(),
                        ".git" | "target" | "node_modules" | "dist" | "__pycache__" | ".venv" | "venv" | ".idea"
                    )
                }
                Err(_) => false,
            });
        });

    let mut arena: Vec<BNode> = Vec::with_capacity(1 << 16);
    arena.push(BNode {
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string()),
        path: root.clone(),
        is_dir: true,
        lang: Lang::Other,
        lines: 0,
        bytes: 0,
        weight: 0.0,
        children: Vec::new(),
    });
    let mut dir_map: HashMap<PathBuf, u32> = HashMap::new();
    dir_map.insert(root.clone(), 0);

    let mut file_ix: Vec<u32> = Vec::new();
    let mut walked = 0u64;

    for entry in walker {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let parent_idx = match ensure_dir(&mut arena, &mut dir_map, path.parent().unwrap_or(&root), &root) {
            Some(i) => i,
            None => continue,
        };
        if entry.file_type.is_dir() {
            ensure_dir(&mut arena, &mut dir_map, &path, &root);
            continue;
        }
        if !entry.file_type.is_file() {
            continue;
        }
        let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let name = entry.file_name.to_string_lossy().into_owned();
        let lang = Lang::from_path(&path);
        let idx = arena.len() as u32;
        arena.push(BNode {
            name,
            path,
            is_dir: false,
            lang,
            lines: 0,
            bytes,
            weight: 0.0,
            children: Vec::new(),
        });
        arena[parent_idx as usize].children.push(idx);
        file_ix.push(idx);
        walked += 1;
        if walked % 5000 == 0 {
            progress.files_walked.store(walked, Ordering::Relaxed);
            progress.bytes.fetch_add(bytes, Ordering::Relaxed);
        }
    }
    progress.files_walked.store(walked, Ordering::Relaxed);
    progress.total_files.store(file_ix.len() as u64, Ordering::Relaxed);

    progress.phase.store(2, Ordering::Relaxed);
    let counted: Vec<(u32, u32, bool)> = file_ix
        .par_iter()
        .map(|&i| {
            let n = &arena[i as usize];
            let (lines, is_bin) = if n.bytes > MAX_COUNT_SIZE || n.lang.is_binary() {
                (0, n.lang.is_binary())
            } else {
                match fs::read(&n.path) {
                    Ok(data) => {
                        if data.iter().take(8192).any(|&b| b == 0) {
                            (0, true)
                        } else {
                            (memchr::memchr_iter(b'\n', &data).count() as u32 + 1, false)
                        }
                    }
                    Err(_) => (0, false),
                }
            };
            progress.files_counted.fetch_add(1, Ordering::Relaxed);
            progress.lines.fetch_add(lines as u64, Ordering::Relaxed);
            (i, lines, is_bin)
        })
        .collect();
    for (i, lines, is_bin) in counted {
        arena[i as usize].lines = lines;
        if is_bin {
            arena[i as usize].lang = Lang::Binary;
        }
    }

    for n in arena.iter_mut() {
        n.weight = if n.is_dir {
            0.0
        } else if n.lang.is_binary() {
            1.0
        } else if n.lines > 0 {
            n.lines as f64
        } else {
            (n.bytes as f64 / 40.0).max(1.0)
        };
    }

    aggregate(&mut arena, 0);

    progress.phase.store(3, Ordering::Relaxed);

    let mut sorted_children: Vec<Vec<u32>> = arena.iter().map(|n| n.children.clone()).collect();
    for ch in sorted_children.iter_mut() {
        ch.sort_by(|&a, &b| arena[b as usize].weight.total_cmp(&arena[a as usize].weight));
    }

    let mut sizes = vec![0usize; arena.len()];
    fn sub_size(sorted: &[Vec<u32>], sizes: &mut [usize], i: u32) -> usize {
        if sizes[i as usize] > 0 {
            return sizes[i as usize];
        }
        let mut s = 1;
        for &c in &sorted[i as usize] {
            s += sub_size(sorted, sizes, c);
        }
        sizes[i as usize] = s;
        s
    }
    let total_nodes = sub_size(&sorted_children, &mut sizes, 0);

    let mut scene_nodes: Vec<Node> = vec![
        Node {
            name_start: 0,
            name_len: 0,
            is_dir: false,
            lang: Lang::Other,
            depth: 0,
            lines: 0,
            weight: 0.0,
            rect: [0.0; 4],
            children_start: 0,
            children_count: 0,
            path_start: 0,
            path_len: 0,
        };
        total_nodes
    ];
    let mut names: Vec<u8> = Vec::new();
    let mut paths: Vec<u8> = Vec::new();
    let mut children_ids: Vec<u32> = Vec::new();
    let mut stats = Stats { files: 0, dirs: 0, lines: 0, bytes: 0 };

    fn place(
        arena: &[BNode],
        sorted: &[Vec<u32>],
        sizes: &[usize],
        i: u32,
        pos: usize,
        depth: u16,
        out: &mut [Node],
        names: &mut Vec<u8>,
        paths: &mut Vec<u8>,
        children_ids: &mut Vec<u32>,
        stats: &mut Stats,
    ) {
        let b = &arena[i as usize];
        if b.is_dir {
            stats.dirs += 1;
        } else {
            stats.files += 1;
            stats.lines += b.lines as u64;
            stats.bytes += b.bytes;
        }
        let children = &sorted[i as usize];
        let mut child_idx: Vec<u32> = Vec::with_capacity(children.len());
        let mut cursor = pos + 1;
        for &c in children {
            place(arena, sorted, sizes, c, cursor, depth + 1, out, names, paths, children_ids, stats);
            child_idx.push(cursor as u32);
            cursor += sizes[c as usize];
        }
        let name_start = names.len() as u32;
        names.extend_from_slice(b.name.as_bytes());
        let path_str = b.path.as_os_str().to_string_lossy();
        let path_start = paths.len() as u32;
        paths.extend_from_slice(path_str.as_bytes());
        let cstart = children_ids.len() as u32;
        children_ids.extend_from_slice(&child_idx);
        out[pos] = Node {
            name_start,
            name_len: b.name.len() as u32,
            is_dir: b.is_dir,
            lang: b.lang,
            depth,
            lines: b.lines,
            weight: b.weight.max(1.0),
            rect: [0.0; 4],
            children_start: cstart,
            children_count: child_idx.len() as u32,
            path_start,
            path_len: path_str.len() as u32,
        };
    }
    place(
        &arena,
        &sorted_children,
        &sizes,
        0,
        0,
        0,
        &mut scene_nodes,
        &mut names,
        &mut paths,
        &mut children_ids,
        &mut stats,
    );
    let root_new = 0;

    let mut scene = Scene { nodes: scene_nodes, children_ids, names, paths, root: root_new, stats };
    crate::layout::squarify(&mut scene, root_new);
    progress.phase.store(4, Ordering::Relaxed);
    scene
}

fn ensure_dir(arena: &mut Vec<BNode>, dir_map: &mut HashMap<PathBuf, u32>, dir: &Path, root: &Path) -> Option<u32> {
    if let Some(&i) = dir_map.get(dir) {
        return Some(i);
    }
    if !dir.starts_with(root) {
        return None;
    }
    let parent = dir.parent()?;
    let parent_idx = ensure_dir(arena, dir_map, parent, root)?;
    let name = dir.file_name()?.to_string_lossy().into_owned();
    let idx = arena.len() as u32;
    arena.push(BNode {
        name,
        path: dir.to_path_buf(),
        is_dir: true,
        lang: Lang::Other,
        lines: 0,
        bytes: 0,
        weight: 0.0,
        children: Vec::new(),
    });
    arena[parent_idx as usize].children.push(idx);
    dir_map.insert(dir.to_path_buf(), idx);
    Some(idx)
}

fn aggregate(arena: &mut Vec<BNode>, i: u32) -> f64 {
    let mut sum = arena[i as usize].weight;
    for k in 0..arena[i as usize].children.len() {
        let c = arena[i as usize].children[k];
        sum += aggregate(arena, c);
    }
    arena[i as usize].weight = sum;
    sum
}

pub fn node_name<'a>(scene: &'a Scene, n: &Node) -> &'a str {
    std::str::from_utf8(&scene.names[n.name_start as usize..(n.name_start + n.name_len) as usize]).unwrap_or("?")
}

pub fn node_path<'a>(scene: &'a Scene, n: &Node) -> &'a str {
    std::str::from_utf8(&scene.paths[n.path_start as usize..(n.path_start + n.path_len) as usize]).unwrap_or("")
}
