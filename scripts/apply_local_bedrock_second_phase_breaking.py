from pathlib import Path

root = Path(__file__).resolve().parents[1]

options_path = root / "crates/bedrock-leveldb/src/options.rs"
options = options_path.read_text(encoding="utf-8")
old_field = "    /// Maximum decoded native table block cache size, in bytes.\n    pub cache_size: usize,\n"
new_field = "    /// Independent sharded native table cache capacities.\n    pub cache: NativeCacheOptions,\n"
if options.count(old_field) != 1:
    raise RuntimeError("legacy OpenOptions::cache_size field was not found exactly once")
options = options.replace(old_field, new_field, 1)
old_default = "            cache_size: 64 * 1024 * 1024,\n"
new_default = "            cache: NativeCacheOptions::default(),\n"
if options.count(old_default) != 1:
    raise RuntimeError("legacy cache_size default was not found exactly once")
options = options.replace(old_default, new_default, 1)
options_path.write_text(options, encoding="utf-8")

db_path = root / "crates/bedrock-leveldb/src/db.rs"
db = db_path.read_text(encoding="utf-8")
old_open = '''    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let cache_options = NativeCacheOptions::from_total(options.cache_size);
        Self::open_with_cache_options(path, options, cache_options)
    }

    /// Opens a database with independent sharded cache capacities.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::open`].
    pub fn open_with_cache_options(
        path: impl AsRef<Path>,
        options: OpenOptions,
        cache_options: NativeCacheOptions,
    ) -> Result<Self> {
        let root = path.as_ref().to_path_buf();'''
new_open = '''    pub fn open(path: impl AsRef<Path>, options: OpenOptions) -> Result<Self> {
        let cache_options = options.cache.normalized();
        let root = path.as_ref().to_path_buf();'''
if db.count(old_open) != 1:
    raise RuntimeError("temporary Db::open_with_cache_options compatibility API was not found")
db = db.replace(old_open, new_open, 1)
old_normalize = "        let cache_options = cache_options.normalized();\n"
if db.count(old_normalize) != 1:
    raise RuntimeError("duplicate cache normalization site was not found")
db = db.replace(old_normalize, "", 1)
db_path.write_text(db, encoding="utf-8")

test_path = root / "crates/bedrock-leveldb/tests/second_phase.rs"
test = test_path.read_text(encoding="utf-8")
old_test = '''    let db = Db::open_with_cache_options(
        dir.path(),
        OpenOptions::default(),
        NativeCacheOptions {
            data_capacity: 1024 * 1024,
            index_capacity: 1024 * 1024,
            file_capacity: 8,
            shards: 4,
        },
    )?;'''
new_test = '''    let db = Db::open(
        dir.path(),
        OpenOptions {
            cache: NativeCacheOptions {
                data_capacity: 1024 * 1024,
                index_capacity: 1024 * 1024,
                file_capacity: 8,
                shards: 4,
            },
            ..OpenOptions::default()
        },
    )?;'''
if test.count(old_test) != 1:
    raise RuntimeError("second phase test still does not contain temporary open API")
test_path.write_text(test.replace(old_test, new_test, 1), encoding="utf-8")

cargo_path = root / "crates/bedrock-leveldb/Cargo.toml"
cargo = cargo_path.read_text(encoding="utf-8")
if 'version = "0.4.0"' not in cargo:
    raise RuntimeError("expected bedrock-leveldb 0.4.0 before breaking API bump")
cargo_path.write_text(cargo.replace('version = "0.4.0"', 'version = "0.5.0"', 1), encoding="utf-8")

changelog_path = root / "crates/bedrock-leveldb/CHANGELOG.md"
if changelog_path.exists():
    changelog = changelog_path.read_text(encoding="utf-8")
    heading = "# Changelog\n"
    entry = "\n## 0.5.0\n\n- Replace the aggregate `OpenOptions::cache_size` setting with independent sharded native cache capacities.\n- Remove the temporary `Db::open_with_cache_options` compatibility entry point.\n- Add compact table identities, bounded file handles, cache statistics, incremental WAL recovery accounting, and allocation-reduced exact batch reads.\n"
    if "## 0.5.0" not in changelog:
        if changelog.startswith(heading):
            changelog = heading + entry + changelog[len(heading):]
        else:
            changelog = entry.lstrip() + "\n" + changelog
        changelog_path.write_text(changelog, encoding="utf-8")
