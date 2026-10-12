//! Formatting and mounting an OJFS v3 volume, replaying its journal and the read-only accessors.

use super::*;

mod inodes;
mod meta;
mod txn;

impl<D: BlockDevice> Fs3<D> {
    /// Create a new, empty filesystem on `dev` (use `&mut dev` to keep the
    /// device on failure). The v3 area starts at LBA 128; sectors 0..127 are
    /// never written. The superblock is written last.
    pub fn format(dev: D, opts: &FormatOptions) -> Result<Self, FsError> {
        let mut fs = Self::format_unsealed(dev, opts)?;
        fs.seal()?;
        Ok(fs)
    }

    /// Build the whole structure but do not write the superblock yet, so the
    /// device still reads as "no v3". Used by `format` and the migration.
    pub(crate) fn format_unsealed(dev: D, opts: &FormatOptions) -> Result<Self, FsError> {
        let sectors = dev.sector_count();
        if sectors < MIN_DISK_SECTORS {
            return Err(FsError::TooSmall);
        }
        let total = blocks_for(sectors);
        let (dj, di) = Geometry::defaults(total);
        let j = opts.journal_blocks.unwrap_or(dj);
        let ino = opts
            .inode_count
            .unwrap_or(di)
            .div_ceil(INODES_PER_BLOCK)
            .checked_mul(INODES_PER_BLOCK)
            .ok_or(FsError::TooSmall)?;
        let geo = Geometry::compute(total, j, ino).ok_or(FsError::TooSmall)?;
        let sb = Superblock {
            uuid: opts.uuid,
            created: opts.now,
            geo,
        };
        let mut cache = BlockCache::new(
            dev,
            FS_START_LBA,
            total as u64,
            opts.cache_blocks.unwrap_or(DEFAULT_CACHE_BLOCKS),
        );
        // 1. Whatever superblocks were there (primary, and the copy where ours
        //    would go) must not survive a half-finished format.
        let zero = [0u8; SECTOR_SIZE];
        cache.dev_mut().write_sectors(FS_START_LBA, &zero)?;
        let backup = FS_START_LBA + geo.backup_sb() as u64 * SECTORS_PER_BLOCK;
        cache.dev_mut().write_sectors(backup, &zero)?;
        cache.dev_mut().flush()?;
        // 2. Empty journal.
        let hdr = build_journal_header(0, &[], &[]);
        cache.write_direct(geo.jhdr as u64, &hdr, 0)?;
        // 3. Bitmaps with the fixed metadata marked used.
        let mut bbits = bits::new(total);
        for b in 0..geo.data_start {
            bits::set(&mut bbits, b);
        }
        bits::set(&mut bbits, geo.backup_sb());
        for i in 0..geo.bbitmap_blocks {
            let img = bitmap_image(&bbits, i);
            cache.write_direct((geo.bbitmap_start + i) as u64, &img, 0)?;
        }
        let mut ibits = bits::new(geo.inode_count);
        bits::set(&mut ibits, ROOT_INO - 1);
        bits::set(&mut ibits, TRASH_INO - 1);
        for i in 0..geo.ibitmap_blocks {
            let img = bitmap_image(&ibits, i);
            cache.write_direct((geo.ibitmap_start + i) as u64, &img, 0)?;
        }
        cache.flush()?;
        let mut fs = Self::open_with_cache(cache, sb, false)?;
        // 4. Root and /.trash.
        let now = opts.now;
        fs.txn(|fs| {
            let mut root = Inode::new(Kind::Dir, now, ROOT_INO);
            root.nlink = 1;
            let trash = Inode::new(Kind::Dir, now, ROOT_INO);
            fs.write_inode(ROOT_INO, &root)?;
            fs.write_inode(TRASH_INO, &trash)?;
            fs.dir_insert(ROOT_INO, TRASH_NAME, TRASH_INO, Kind::Dir, now)
        })?;
        fs.validate_roots()?;
        Ok(fs)
    }

    /// Make the filesystem visible: flush everything, then write the superblock
    /// (a single sector, so it is either wholly there or absent) and its copy.
    pub(crate) fn seal(&mut self) -> Result<(), FsError> {
        self.cache.flush()?;
        let enc = self.sb.encode();
        let backup = FS_START_LBA + self.geo.backup_sb() as u64 * SECTORS_PER_BLOCK;
        self.cache.dev_mut().write_sectors(FS_START_LBA, &enc)?;
        self.cache.dev_mut().flush()?;
        self.cache.dev_mut().write_sectors(backup, &enc)?;
        self.cache.dev_mut().flush()?;
        Ok(())
    }

    /// Mount with the default cache size. Replays the journal, validates the
    /// superblock, bitmaps, journal, root and trash. Never panics on a corrupt
    /// disk; returns [`FsError::BadSuperblock`] or [`FsError::Corrupt`].
    pub fn mount(dev: D) -> Result<Self, FsError> {
        Self::mount_with(dev, DEFAULT_CACHE_BLOCKS)
    }

    /// [`mount`](Self::mount) with a cache of `cache_blocks` 4 KiB blocks.
    pub fn mount_with(mut dev: D, cache_blocks: usize) -> Result<Self, FsError> {
        let sb = match read_sb_state(&mut dev)? {
            SbState::Valid(sb) => sb,
            _ => return Err(FsError::BadSuperblock),
        };
        let cache = BlockCache::new(dev, FS_START_LBA, sb.geo.total_blocks as u64, cache_blocks);
        let mut fs = Self::open_with_cache(cache, sb, true)?;
        fs.validate_roots()?;
        Ok(fs)
    }

    /// [`mount`](Self::mount) followed by a full [`fsck`](Self::fsck); fails
    /// with `Corrupt` if the check finds anything.
    pub fn mount_verified(dev: D) -> Result<Self, FsError> {
        let mut fs = Self::mount(dev)?;
        let rep = fs.fsck()?;
        if rep.is_clean() {
            Ok(fs)
        } else {
            Err(FsError::Corrupt("fsck found problems"))
        }
    }

    pub(super) fn open_with_cache(
        mut cache: BlockCache<D>,
        sb: Superblock,
        _check: bool,
    ) -> Result<Self, FsError> {
        let geo = sb.geo;
        if cache.block_count() < geo.total_blocks as u64 {
            return Err(FsError::BadSuperblock);
        }
        let seq = Self::replay_journal(&mut cache, &geo)?;
        // Bitmaps, read once, straight from the device.
        let mut raw = alloc::vec![0u8; geo.bbitmap_blocks as usize * BLOCK_SIZE];
        cache.read_many(geo.bbitmap_start as u64, &mut raw)?;
        let mut bbits = bits::new(geo.total_blocks);
        for i in 0..geo.bbitmap_blocks {
            let o = i as usize * BLOCK_SIZE;
            load_bitmap_block(&raw[o..o + BLOCK_SIZE], i, &mut bbits)?;
        }
        bits::fix_padding(&mut bbits, geo.total_blocks);
        let mut raw = alloc::vec![0u8; geo.ibitmap_blocks as usize * BLOCK_SIZE];
        cache.read_many(geo.ibitmap_start as u64, &mut raw)?;
        let mut ibits = bits::new(geo.inode_count);
        for i in 0..geo.ibitmap_blocks {
            let o = i as usize * BLOCK_SIZE;
            load_bitmap_block(&raw[o..o + BLOCK_SIZE], i, &mut ibits)?;
        }
        bits::fix_padding(&mut ibits, geo.inode_count);
        // The fixed metadata must be marked used.
        if bits::next_free(&bbits, 0, geo.data_start).is_some()
            || !bits::get(&bbits, geo.backup_sb())
        {
            return Err(FsError::Corrupt("bitmap leaves the metadata uncovered"));
        }
        if !bits::get(&ibits, ROOT_INO - 1) || !bits::get(&ibits, TRASH_INO - 1) {
            return Err(FsError::Corrupt("root/trash inode not allocated"));
        }
        let free_blocks = bits::count_free(&bbits, geo.total_blocks);
        let free_inodes = bits::count_free(&ibits, geo.inode_count);
        Ok(Fs3 {
            cache,
            sb,
            geo,
            bbits,
            ibits,
            free_blocks,
            free_inodes,
            alloc_hint: geo.data_start,
            seq,
            tx: Tx::idle(),
            poisoned: false,
            commits: 0,
        })
    }

    /// Replay a committed-but-not-checkpointed transaction. Idempotent.
    /// Returns the journal sequence number.
    pub(super) fn replay_journal(
        cache: &mut BlockCache<D>,
        geo: &Geometry,
    ) -> Result<u64, FsError> {
        let mut h = [0u8; BLOCK_SIZE];
        cache.read(geo.jhdr as u64, &mut h)?;
        let Some(parsed) = parse_journal_header(&h, geo.journal_blocks) else {
            return Ok(0);
        };
        let n = parsed.targets.len();
        if n == 0 {
            return Ok(parsed.seq);
        }
        let mut payload = alloc::vec![0u8; n * BLOCK_SIZE];
        cache.read_many(geo.jpayload as u64, &mut payload)?;
        if !journal_crc_ok(&h, &payload) {
            // A torn commit record: the transaction never committed.
            return Ok(parsed.seq);
        }
        // A target must be a metadata block that lives outside the journal and
        // the superblocks (a checksum-valid but hostile record must not be
        // able to overwrite them).
        for &t in &parsed.targets {
            if t < geo.bbitmap_start || t >= geo.backup_sb() {
                return Err(FsError::Corrupt("journal target out of range"));
            }
        }
        for (i, &t) in parsed.targets.iter().enumerate() {
            let mut blk = [0u8; BLOCK_SIZE];
            blk.copy_from_slice(&payload[i * BLOCK_SIZE..(i + 1) * BLOCK_SIZE]);
            cache.write_direct(t as u64, &blk, 0)?;
        }
        cache.flush()?;
        let clear = build_journal_header(parsed.seq, &[], &[]);
        cache.write_direct(geo.jhdr as u64, &clear, 0)?;
        cache.flush()?;
        Ok(parsed.seq)
    }

    pub(super) fn validate_roots(&mut self) -> Result<(), FsError> {
        let root = self.read_inode(ROOT_INO)?;
        if root.kind != Kind::Dir || root.parent != ROOT_INO {
            return Err(FsError::Corrupt("bad root inode"));
        }
        let trash = self.read_inode(TRASH_INO)?;
        if trash.kind != Kind::Dir || trash.parent != ROOT_INO {
            return Err(FsError::Corrupt("bad trash inode"));
        }
        match self.dir_find(ROOT_INO, &root, TRASH_NAME)? {
            Some((TRASH_INO, Kind::Dir)) => Ok(()),
            _ => Err(FsError::Corrupt("root has no .trash entry")),
        }
    }

    /// Give back the device. Nothing is pending: every returned operation is
    /// already durable.
    pub fn into_device(self) -> D {
        self.cache.into_inner()
    }

    /// Borrow the device.
    pub fn device(&self) -> &D {
        self.cache.dev()
    }

    /// Mutably borrow the device (bypasses the cache; for tests).
    pub fn device_mut(&mut self) -> &mut D {
        self.cache.dev_mut()
    }

    /// Block-cache counters.
    pub fn cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// Zero the block-cache counters.
    pub fn reset_cache_stats(&mut self) {
        self.cache.reset_stats();
    }

    /// Journal transactions committed since mount.
    pub fn commits(&self) -> u64 {
        self.commits
    }

    /// The superblock (UUID, creation time, geometry).
    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// Capacity and free space.
    pub fn statfs(&self) -> StatFs {
        StatFs {
            block_size: BLOCK_SIZE as u32,
            total_blocks: self.geo.total_blocks,
            free_blocks: self.free_blocks,
            data_blocks: self.geo.total_blocks - self.geo.data_start - 1,
            total_inodes: self.geo.inode_count,
            free_inodes: self.free_inodes,
        }
    }

    /// Force a device barrier. Operations are already durable when they
    /// return; this exists for callers that want an explicit sync point.
    pub fn sync(&mut self) -> Result<(), FsError> {
        self.ready()?;
        self.cache.flush()?;
        Ok(())
    }
}
