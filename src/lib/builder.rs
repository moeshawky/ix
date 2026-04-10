//! Index builder — the complete pipeline from files to .ix shard.
//!
//! Phase 1: Discovery (walk directory, respect .gitignore)
//! Phase 2: Scan (mmap, check binary, extract trigrams, bloom filter)
//! Phase 3: Serialize (write sections, compute CRCs, atomic rename)

use crate::bloom::BloomFilter;
use crate::decompress::maybe_decompress;
use crate::error::Result;
use crate::format::*;
use crate::posting::{PostingEntry, PostingList};
use crate::trigram::{Extractor, Trigram};
use ignore::WalkBuilder;
use libc;
use llmosafe::{ResourceGuard, WorkingMemory, sift_perceptions};
use memmap2::Mmap;
use std::collections::{BinaryHeap, HashMap};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub struct Builder {
    root: PathBuf,
    ix_dir: PathBuf,
    file_count: u32,

    // O(1) memory streaming writers for temporary file table and blooms
    files_writer: BufWriter<File>,
    blooms_writer: BufWriter<File>,
    strings_writer: BufWriter<File>,

    // Postings batching for external sort
    postings: HashMap<Trigram, Vec<PostingEntry>>,
    postings_count: usize,
    temp_runs: Vec<PathBuf>,

    extractor: Extractor,
    stats: BuildStats,
    decompress: bool,
    resource_guard: Option<ResourceGuard>,
    cognitive_memory: WorkingMemory<128>,
    dead_ends: Vec<PathBuf>,
}

#[derive(Default, Debug)]
pub struct BuildStats {
    pub files_scanned: u64,
    pub files_skipped_binary: u64,
    pub files_skipped_size: u64,
    pub bytes_scanned: u64,
    pub unique_trigrams: u64,
}

struct RunIterator {
    file: BufReader<File>,
}

impl RunIterator {
    fn new(path: &Path) -> Result<Self> {
        let f = File::open(path)?;
        Ok(Self {
            file: BufReader::new(f),
        })
    }

    fn next_trigram(&mut self) -> Result<Option<(Trigram, Vec<PostingEntry>)>> {
        let mut tri_buf = [0u8; 4];
        if let Err(e) = self.file.read_exact(&mut tri_buf) {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                return Ok(None);
            }
            return Err(e.into());
        }
        let tri = u32::from_le_bytes(tri_buf);

        let mut len_buf = [0u8; 4];
        self.file.read_exact(&mut len_buf)?;
        let entries_len = u32::from_le_bytes(len_buf) as usize;

        let mut entries = Vec::with_capacity(entries_len);
        for _ in 0..entries_len {
            self.file.read_exact(&mut len_buf)?;
            let file_id = u32::from_le_bytes(len_buf);

            self.file.read_exact(&mut len_buf)?;
            let offsets_len = u32::from_le_bytes(len_buf) as usize;

            let mut offsets = Vec::with_capacity(offsets_len);
            for _ in 0..offsets_len {
                self.file.read_exact(&mut len_buf)?;
                offsets.push(u32::from_le_bytes(len_buf));
            }
            entries.push(PostingEntry { file_id, offsets });
        }

        Ok(Some((tri, entries)))
    }
}

#[derive(Eq, PartialEq)]
struct MergeItem {
    tri: Trigram,
    run_idx: usize,
}

impl PartialOrd for MergeItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MergeItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.tri.cmp(&self.tri) // Min-heap
    }
}

impl Builder {
    pub fn new(root: &Path) -> Result<Self> {
        let ix_dir = root.join(".ix");
        fs::create_dir_all(&ix_dir)?;

        let files_tmp = ix_dir.join("shard.ix.tmp.files");
        let blooms_tmp = ix_dir.join("shard.ix.tmp.blooms");
        let strings_tmp = ix_dir.join("shard.ix.tmp.strings");

        let files_writer = BufWriter::new(File::create(&files_tmp)?);
        let blooms_writer = BufWriter::new(File::create(&blooms_tmp)?);
        let mut strings_writer = BufWriter::new(File::create(&strings_tmp)?);

        strings_writer.write_all(&1u32.to_le_bytes())?;
        strings_writer.write_all(&0u16.to_le_bytes())?;
        strings_writer.write_all(&0u16.to_le_bytes())?;
        strings_writer.write_all(&[0u8; 2])?;

        Ok(Self {
            root: root.to_owned(),
            ix_dir,
            file_count: 0,
            files_writer,
            blooms_writer,
            strings_writer,
            postings: HashMap::new(),
            postings_count: 0,
            temp_runs: Vec::new(),
            extractor: Extractor::new(),
            stats: BuildStats::default(),
            decompress: false,
            resource_guard: None,
            cognitive_memory: WorkingMemory::new(1000), // Standard surprise threshold
            dead_ends: Vec::new(),
        })
    }

    pub fn with_resource_guard(mut self, guard: ResourceGuard) -> Self {
        self.resource_guard = Some(guard);
        self
    }

    pub fn set_decompress(&mut self, decompress: bool) {
        self.decompress = decompress;
    }

    fn flush_run(&mut self) -> Result<()> {
        if self.postings.is_empty() {
            return Ok(());
        }
        let old_postings = std::mem::take(&mut self.postings);
        let mut sorted: Vec<_> = old_postings.into_iter().collect();
        sorted.sort_unstable_by_key(|(t, _)| *t);

        let run_path = self
            .ix_dir
            .join(format!("shard.ix.run.{}", self.temp_runs.len()));
        let mut f = BufWriter::new(File::create(&run_path)?);

        for (tri, entries) in sorted {
            f.write_all(&tri.to_le_bytes())?;
            f.write_all(&(entries.len() as u32).to_le_bytes())?;
            for entry in entries {
                f.write_all(&entry.file_id.to_le_bytes())?;
                f.write_all(&(entry.offsets.len() as u32).to_le_bytes())?;
                for off in entry.offsets {
                    f.write_all(&off.to_le_bytes())?;
                }
            }
        }
        f.flush()?;

        self.temp_runs.push(run_path);
        self.postings_count = 0;
        Ok(())
    }

    pub fn build(&mut self) -> Result<PathBuf> {
        let start = Instant::now();
        let root = self.root.clone();

        // LLMOSafe Formal Law: Sensitive filesystem traversal (Root)
        if root.to_string_lossy() == "/" {
            tracing::warn!(
                "LLMOSafe Advisory: Indexing root filesystem. Ensure adequate resource guards are in place."
            );
        }

        let walker = WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(true)
            .require_git(false)
            .add_custom_ignore_filename(".ixignore")
            .filter_entry(move |entry| {
                let path = entry.path();
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false)
                    && (name == "lost+found"
                        || name == ".git"
                        || name == "node_modules"
                        || name == "target"
                        || name == "__pycache__"
                        || name == ".tox"
                        || name == ".venv"
                        || name == "venv"
                        || name == ".ix")
                {
                    return false;
                }

                if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    if let Ok(metadata) = entry.metadata()
                        && metadata.len() > 10 * 1024 * 1024
                    {
                        return false;
                    }
                    if name == "Cargo.lock"
                        || name == "package-lock.json"
                        || name == "pnpm-lock.yaml"
                        || name == "shard.ix"
                        || name == "shard.ix.tmp"
                        || name.starts_with("shard.ix.")
                    {
                        return false;
                    }
                }

                if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    match ext {
                        "so" | "o" | "dylib" | "a" | "dll" | "exe" | "pyc" | "jpg" | "png"
                        | "gif" | "mp4" | "mp3" | "pdf" | "zip" | "7z" | "rar" | "sqlite"
                        | "db" | "bin" => return false,
                        _ => {}
                    }
                    if name.ends_with(".tar.gz") {
                        return false;
                    }
                }
                true
            })
            .build();

        let mut files_processed = 0u64;
        for entry_res in walker {
            let entry = match entry_res {
                Ok(e) => e,
                Err(e) => {
                    // Handle KernelError::BacktrackSignaled (-7) during the walk
                    let backtrack_path = match &e {
                        ignore::Error::Io(io_err) if io_err.raw_os_error() == Some(-7) => {
                            Some(None)
                        }
                        ignore::Error::WithPath { path, err } => {
                            if let ignore::Error::Io(io_err) = err.as_ref() {
                                if io_err.raw_os_error() == Some(-7) {
                                    Some(Some(path.clone()))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };

                    if let Some(path_opt) = backtrack_path {
                        tracing::warn!(
                            "Immune Memory Triggered: Skipping path due to backtrack signal."
                        );
                        if let Some(path) = path_opt {
                            self.dead_ends.push(path);
                        }
                    }
                    continue;
                }
            };

            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                self.process_file(entry.path().to_owned())?;
                files_processed += 1;

                // Resource Guard Check: check every 250 files to prevent OOM
                if files_processed.is_multiple_of(250) {
                    if let Some(guard) = &self.resource_guard {
                        if guard.check().map(|_s: ::llmosafe::Synapse| ()).is_err() {
                            let _err = guard.check().unwrap_err();
                            eprintln!(
                                "ixd: memory ceiling reached... flushing intermediate chunk ({} files processed)",
                                files_processed
                            );
                            self.flush_run()?;
                            continue;
                        }
                    } else {
                        // Fallback to manual RSS limit if no formal guard provided
                        if let Ok(rss) = Self::current_rss_bytes()
                            && rss > 512 * 1024 * 1024
                        {
                            eprintln!(
                                "ixd: RSS ceiling reached ({} MB) after {} files — flushing intermediate chunk",
                                rss / 1024 / 1024,
                                files_processed
                            );
                            self.flush_run()?;
                            continue;
                        }
                    }
                }
            }
        }

        let output_path = self.serialize()?;
        tracing::info!("Build completed in {:?}: {:?}", start.elapsed(), self.stats);
        Ok(output_path)
    }

    pub fn update(&mut self, _changed_files: &[PathBuf]) -> Result<PathBuf> {
        self.build()
    }

    pub fn files_len(&self) -> usize {
        self.file_count as usize
    }

    pub fn trigrams_len(&self) -> usize {
        self.stats.unique_trigrams as usize
    }

    /// Returns current process RSS in bytes by reading /proc/self/status.
    fn current_rss_bytes() -> std::io::Result<u64> {
        let status = std::fs::read_to_string("/proc/self/status")?;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                let kb: u64 = rest
                    .split_whitespace()
                    .next()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                return Ok(kb * 1024);
            }
        }
        Ok(0)
    }

    /// Returns free bytes available on the filesystem containing `path`.
    fn free_bytes_at(path: &Path) -> std::io::Result<u64> {
        use std::os::unix::ffi::OsStrExt;
        let path_c = std::ffi::CString::new(path.as_os_str().as_bytes())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        let ret = unsafe { libc::statvfs(path_c.as_ptr(), &mut stat) };
        if ret != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(stat.f_bavail * stat.f_frsize)
    }

    fn process_file(&mut self, path: PathBuf) -> Result<bool> {
        // TOCTOU guard: file may have been deleted between walk and open
        let metadata = match fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e.into()),
        };
        let size = metadata.len();
        let mtime = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);

        if size > 10 * 1024 * 1024 {
            self.stats.files_skipped_size += 1;
            return Ok(false);
        }

        let file = match File::open(&path) {
            Ok(f) => f,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        };
        let mmap = unsafe { Mmap::map(&file)? };

        let raw_data = if self.decompress {
            if let Some(mut reader) = maybe_decompress(&path, &mmap)? {
                let mut buf = Vec::new();
                use std::io::Read;
                reader
                    .by_ref()
                    .take(10 * 1024 * 1024)
                    .read_to_end(&mut buf)?;
                std::borrow::Cow::Owned(buf)
            } else {
                std::borrow::Cow::Borrowed(&mmap[..])
            }
        } else {
            std::borrow::Cow::Borrowed(&mmap[..])
        };

        let data = &raw_data[..];
        if is_binary(data) {
            self.stats.files_skipped_binary += 1;
            return Ok(false);
        }

        // LLMOSafe Tier 3: Perceptual Sifting (Cognitive Layer)
        // Evaluate file utility and bias (Halo signal)
        let sample_len = data.len().min(2048);
        let sample = String::from_utf8_lossy(&data[..sample_len]);
        let objective = "High-signal source code for semantic indexing";
        let sifted = sift_perceptions(&[sample.as_ref()], objective);

        if let Err(e) = self.cognitive_memory.update(sifted) {
            tracing::warn!(
                "LLMOSafe Cognitive Guard rejection for {}: {:?}",
                path.display(),
                e
            );
            // Skip files that don't pass the safety/utility check (e.g., high bias/halo or high surprise)
            return Ok(false);
        }

        let content_hash = xxhash_rust::xxh64::xxh64(data, 0);
        let pairs = self.extractor.extract_with_offsets(data);

        let file_id = self.file_count;
        self.file_count += 1;

        let path_str = path.to_string_lossy();
        let path_bytes = path_str.as_bytes();
        let path_off = (self.strings_writer.stream_position()?) as u32;
        let path_len = path_bytes.len() as u16;

        self.strings_writer.write_all(&0u16.to_le_bytes())?;
        self.strings_writer.write_all(&path_len.to_le_bytes())?;
        self.strings_writer.write_all(path_bytes)?;

        let mut bloom = BloomFilter::new(256, 5);
        let mut trigram_count = 0u32;

        let mut i = 0;
        while i < pairs.len() && trigram_count < 20_000 {
            let tri = pairs[i].0;
            let mut j = i + 1;
            while j < pairs.len() && pairs[j].0 == tri {
                j += 1;
            }

            let take_count = (j - i).min(10_000);
            let offsets: Vec<u32> = pairs[i..i + take_count].iter().map(|p| p.1).collect();

            bloom.insert(tri);
            self.postings
                .entry(tri)
                .or_default()
                .push(PostingEntry { file_id, offsets });
            self.postings_count += take_count + 8;

            trigram_count += 1;
            i = j;
        }

        bloom.serialize(&mut self.blooms_writer)?;

        let bloom_offset = file_id * 260;
        self.files_writer.write_all(&file_id.to_le_bytes())?;
        self.files_writer.write_all(&path_off.to_le_bytes())?;
        self.files_writer.write_all(&path_len.to_le_bytes())?;
        self.files_writer.write_all(&[FileStatus::Fresh as u8])?;
        self.files_writer.write_all(&[0u8])?;
        self.files_writer.write_all(&mtime.to_le_bytes())?;
        self.files_writer.write_all(&size.to_le_bytes())?;
        self.files_writer.write_all(&content_hash.to_le_bytes())?;
        self.files_writer.write_all(&trigram_count.to_le_bytes())?;
        self.files_writer.write_all(&bloom_offset.to_le_bytes())?;
        self.files_writer.write_all(&[0u8; 4])?;

        self.stats.files_scanned += 1;
        self.stats.bytes_scanned += size;

        // Flush every 500k entries (~8MB peak RAM) to prevent unbounded HashMap growth.
        // This was the RAM DDOS root cause in v0.1.1 — threshold was 5M (far too high).
        if self.postings_count >= 500_000 {
            self.flush_run()?;
        }

        Ok(true)
    }

    fn serialize(&mut self) -> Result<PathBuf> {
        // Disk space guard: abort if < 100MB free to avoid partial shard writes
        if let Ok(free) = Self::free_bytes_at(&self.ix_dir) {
            const MIN_FREE: u64 = 100 * 1024 * 1024; // 100 MB
            if free < MIN_FREE {
                return Err(crate::error::Error::Io(std::io::Error::other(format!(
                    "insufficient disk space: {} MB free, need ≥100 MB (path: {})",
                    free / 1024 / 1024,
                    self.ix_dir.display()
                ))));
            }
        }
        self.flush_run()?;

        self.files_writer.flush()?;
        self.blooms_writer.flush()?;
        self.strings_writer.flush()?;

        // Hierarchical Merge to stay under ulimit
        while self.temp_runs.len() > 128 {
            let mut next_generation = Vec::new();
            for chunk in self.temp_runs.chunks(128) {
                let out_path = self.ix_dir.join(format!(
                    "shard.ix.merged.{}.{}",
                    next_generation.len(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_micros()
                ));
                self.merge_to_run(chunk, &out_path)?;
                next_generation.push(out_path);
                for p in chunk {
                    let _ = fs::remove_file(p);
                }
            }
            self.temp_runs = next_generation;
        }

        let tmp_path = self.ix_dir.join("shard.ix.tmp");
        let final_path = self.ix_dir.join("shard.ix");
        let temp_trigrams_path = self.ix_dir.join("shard.ix.tmp.trigrams");

        let mut f = BufWriter::new(File::create(&tmp_path)?);
        f.write_all(&[0u8; HEADER_SIZE])?;

        let file_table_offset = self.align_to_8(&mut f)?;
        let mut files_reader = File::open(self.ix_dir.join("shard.ix.tmp.files"))?;
        std::io::copy(&mut files_reader, &mut f)?;
        let file_table_size = f.stream_position()? - file_table_offset;

        self.align_to_8(&mut f)?;
        let posting_data_offset = f.stream_position()?;

        let mut trigram_table_writer = BufWriter::new(File::create(&temp_trigrams_path)?);
        let mut global_trigram_count = 0u32;

        let mut runs = Vec::new();
        for path in &self.temp_runs {
            runs.push(RunIterator::new(path)?);
        }

        let mut heap = BinaryHeap::new();
        let mut current_items = vec![None; runs.len()];

        for (i, run) in runs.iter_mut().enumerate() {
            if let Some(item) = run.next_trigram()? {
                heap.push(MergeItem {
                    tri: item.0,
                    run_idx: i,
                });
                current_items[i] = Some(item);
            }
        }

        let mut current_tri: Option<Trigram> = None;
        let mut merged_entries: Vec<PostingEntry> = Vec::new();

        while let Some(MergeItem { tri, run_idx }) = heap.pop() {
            if Some(tri) != current_tri {
                if let Some(t) = current_tri {
                    self.write_merged_posting(
                        &mut f,
                        &mut trigram_table_writer,
                        t,
                        posting_data_offset,
                        &mut merged_entries,
                    )?;
                    global_trigram_count += 1;
                    merged_entries.clear();
                }
                current_tri = Some(tri);
            }

            let item = current_items[run_idx].take().unwrap();
            merged_entries.extend(item.1);

            if let Some(next_item) = runs[run_idx].next_trigram()? {
                heap.push(MergeItem {
                    tri: next_item.0,
                    run_idx,
                });
                current_items[run_idx] = Some(next_item);
            }
        }

        if let Some(t) = current_tri {
            self.write_merged_posting(
                &mut f,
                &mut trigram_table_writer,
                t,
                posting_data_offset,
                &mut merged_entries,
            )?;
            global_trigram_count += 1;
        }

        self.stats.unique_trigrams = global_trigram_count as u64;
        let posting_data_size = f.stream_position()? - posting_data_offset;

        self.align_to_8(&mut f)?;
        let trigram_table_offset = f.stream_position()?;
        trigram_table_writer.flush()?;
        drop(trigram_table_writer);

        let mut trigram_table_file = File::open(&temp_trigrams_path)?;
        std::io::copy(&mut trigram_table_file, &mut f)?;
        let trigram_table_size = f.stream_position()? - trigram_table_offset;

        self.align_to_8(&mut f)?;
        let bloom_offset = f.stream_position()?;
        let mut blooms_reader = File::open(self.ix_dir.join("shard.ix.tmp.blooms"))?;
        std::io::copy(&mut blooms_reader, &mut f)?;
        let bloom_size = f.stream_position()? - bloom_offset;

        self.align_to_8(&mut f)?;
        let string_pool_offset = f.stream_position()?;
        let mut strings_reader = File::open(self.ix_dir.join("shard.ix.tmp.strings"))?;
        std::io::copy(&mut strings_reader, &mut f)?;
        let string_pool_size = f.stream_position()? - string_pool_offset;

        let name_index_offset = f.stream_position()?;
        let name_index_size = 0u64;

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;
        let mut header_bytes = [0u8; HEADER_SIZE];
        header_bytes[0..4].copy_from_slice(&MAGIC);
        header_bytes[0x04..0x06].copy_from_slice(&VERSION_MAJOR.to_le_bytes());
        header_bytes[0x06..0x08].copy_from_slice(&VERSION_MINOR.to_le_bytes());
        header_bytes[0x08..0x10].copy_from_slice(
            &(flags::HAS_BLOOM_FILTERS
                | flags::HAS_CONTENT_HASHES
                | flags::POSTING_LISTS_CHECKSUMMED)
                .to_le_bytes(),
        );
        header_bytes[0x10..0x18].copy_from_slice(&created_at.to_le_bytes());
        header_bytes[0x18..0x20].copy_from_slice(&self.stats.bytes_scanned.to_le_bytes());
        header_bytes[0x20..0x24].copy_from_slice(&self.file_count.to_le_bytes());
        header_bytes[0x24..0x28].copy_from_slice(&(global_trigram_count).to_le_bytes());
        header_bytes[0x28..0x30].copy_from_slice(&file_table_offset.to_le_bytes());
        header_bytes[0x30..0x38].copy_from_slice(&file_table_size.to_le_bytes());
        header_bytes[0x38..0x40].copy_from_slice(&trigram_table_offset.to_le_bytes());
        header_bytes[0x40..0x48].copy_from_slice(&trigram_table_size.to_le_bytes());
        header_bytes[0x48..0x50].copy_from_slice(&posting_data_offset.to_le_bytes());
        header_bytes[0x50..0x58].copy_from_slice(&posting_data_size.to_le_bytes());
        header_bytes[0x58..0x60].copy_from_slice(&bloom_offset.to_le_bytes());
        header_bytes[0x60..0x68].copy_from_slice(&bloom_size.to_le_bytes());
        header_bytes[0x68..0x70].copy_from_slice(&string_pool_offset.to_le_bytes());
        header_bytes[0x70..0x78].copy_from_slice(&string_pool_size.to_le_bytes());
        header_bytes[0x78..0x80].copy_from_slice(&name_index_offset.to_le_bytes());
        header_bytes[0x80..0x88].copy_from_slice(&name_index_size.to_le_bytes());

        let crc = crc32c::crc32c(&header_bytes[0..0xF8]);
        header_bytes[0xF8..0xFC].copy_from_slice(&crc.to_le_bytes());

        f.seek(SeekFrom::Start(0))?;
        f.write_all(&header_bytes)?;
        f.flush()?;
        drop(f);

        fs::rename(&tmp_path, &final_path)?;

        let _ = fs::remove_file(self.ix_dir.join("shard.ix.tmp.files"));
        let _ = fs::remove_file(self.ix_dir.join("shard.ix.tmp.blooms"));
        let _ = fs::remove_file(self.ix_dir.join("shard.ix.tmp.strings"));
        let _ = fs::remove_file(&temp_trigrams_path);
        for path in &self.temp_runs {
            let _ = fs::remove_file(path);
        }
        self.temp_runs.clear();

        Ok(final_path)
    }

    fn merge_to_run(&self, run_paths: &[PathBuf], out_path: &Path) -> Result<()> {
        let mut runs = Vec::new();
        for path in run_paths {
            runs.push(RunIterator::new(path)?);
        }
        let mut heap = BinaryHeap::new();
        let mut current_items = vec![None; runs.len()];
        for (i, run) in runs.iter_mut().enumerate() {
            if let Some(item) = run.next_trigram()? {
                heap.push(MergeItem {
                    tri: item.0,
                    run_idx: i,
                });
                current_items[i] = Some(item);
            }
        }
        let mut out = BufWriter::new(File::create(out_path)?);
        let mut current_tri: Option<Trigram> = None;
        let mut merged_entries: Vec<PostingEntry> = Vec::new();
        while let Some(MergeItem { tri, run_idx }) = heap.pop() {
            if Some(tri) != current_tri {
                if let Some(t) = current_tri {
                    self.write_run_entry(&mut out, t, &mut merged_entries)?;
                    merged_entries.clear();
                }
                current_tri = Some(tri);
            }
            let item = current_items[run_idx].take().unwrap();
            merged_entries.extend(item.1);
            if let Some(next_item) = runs[run_idx].next_trigram()? {
                heap.push(MergeItem {
                    tri: next_item.0,
                    run_idx,
                });
                current_items[run_idx] = Some(next_item);
            }
        }
        if let Some(t) = current_tri {
            self.write_run_entry(&mut out, t, &mut merged_entries)?;
        }
        out.flush()?;
        Ok(())
    }

    fn write_run_entry<W: Write>(
        &self,
        w: &mut W,
        tri: Trigram,
        entries: &mut [PostingEntry],
    ) -> Result<()> {
        entries.sort_by_key(|e| e.file_id);
        w.write_all(&tri.to_le_bytes())?;
        w.write_all(&(entries.len() as u32).to_le_bytes())?;
        for entry in entries {
            w.write_all(&entry.file_id.to_le_bytes())?;
            w.write_all(&(entry.offsets.len() as u32).to_le_bytes())?;
            for off in &entry.offsets {
                w.write_all(&off.to_le_bytes())?;
            }
        }
        Ok(())
    }

    fn write_merged_posting<W: Write + Seek>(
        &self,
        f: &mut W,
        table: &mut W,
        tri: Trigram,
        base_off: u64,
        entries: &mut [PostingEntry],
    ) -> Result<()> {
        entries.sort_by_key(|e| e.file_id);
        let count = entries.len() as u32;
        let list = PostingList {
            entries: entries.to_vec(),
        };
        let encoded = list.encode();
        let offset = f.stream_position()? - base_off;
        f.write_all(&encoded)?;
        let abs_off = base_off + offset;
        table.write_all(&tri.to_le_bytes())?;
        table.write_all(&abs_off.to_le_bytes()[..6])?;
        table.write_all(&(encoded.len() as u32).to_le_bytes())?;
        table.write_all(&count.to_le_bytes())?;
        table.write_all(&[0u8; 2])?;
        Ok(())
    }

    fn align_to_8<W: Write + Seek>(&self, mut w: W) -> std::io::Result<u64> {
        let pos = w.stream_position()?;
        let padding = (8 - (pos % 8)) % 8;
        if padding > 0 {
            w.write_all(&vec![0u8; padding as usize])?;
        }
        w.stream_position()
    }
}
