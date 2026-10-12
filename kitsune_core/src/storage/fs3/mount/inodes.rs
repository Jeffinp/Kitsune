//! Inodes and the block allocator: locating, reading and writing inodes, marking and allocating block ranges.

use super::*;

impl<D: BlockDevice> Fs3<D> {
    pub(in super::super) fn inode_loc(&self, ino: Ino) -> Result<(u32, usize), FsError> {
        if ino == 0 || ino > self.geo.inode_count {
            return Err(FsError::NotFound);
        }
        let slot = ino - 1;
        Ok((
            self.geo.itable_start + slot / INODES_PER_BLOCK,
            (slot % INODES_PER_BLOCK) as usize * INODE_SIZE,
        ))
    }

    /// Read a live inode: `NotFound` if `ino` is out of range or unallocated.
    pub(crate) fn read_inode(&mut self, ino: Ino) -> Result<Inode, FsError> {
        let (blk, off) = self.inode_loc(ino)?;
        if !bits::get(&self.ibits, ino - 1) {
            return Err(FsError::NotFound);
        }
        let b = self.meta_get(blk)?;
        Inode::decode(&b[off..off + INODE_SIZE]).ok_or(FsError::Corrupt("inode checksum"))
    }

    pub(crate) fn write_inode(&mut self, ino: Ino, node: &Inode) -> Result<(), FsError> {
        let (blk, off) = self.inode_loc(ino)?;
        let b = self.meta_mut(blk)?;
        node.encode(&mut b[off..off + INODE_SIZE]);
        Ok(())
    }

    pub(crate) fn alloc_inode(&mut self) -> Result<Ino, FsError> {
        let bit = bits::next_free(&self.ibits, 0, self.geo.inode_count).ok_or(FsError::NoInodes)?;
        bits::set(&mut self.ibits, bit);
        self.tx.ilog.push((bit, true));
        self.tx.touched_ib.insert(bit / BITS_PER_BITMAP_BLOCK);
        self.free_inodes = self.free_inodes.saturating_sub(1);
        Ok(bit + 1)
    }

    pub(crate) fn free_inode(&mut self, ino: Ino) -> Result<(), FsError> {
        if ino == ROOT_INO || ino == TRASH_INO {
            return Err(FsError::Corrupt("freeing a fixed inode"));
        }
        let (blk, off) = self.inode_loc(ino)?;
        if !bits::get(&self.ibits, ino - 1) {
            return Err(FsError::Corrupt("double free of an inode"));
        }
        let b = self.meta_mut(blk)?;
        b[off..off + INODE_SIZE].fill(0);
        bits::clear(&mut self.ibits, ino - 1);
        self.tx.ilog.push((ino - 1, false));
        self.tx.touched_ib.insert((ino - 1) / BITS_PER_BITMAP_BLOCK);
        self.free_inodes += 1;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Block allocation (in the RAM bitmap; logged for rollback)
    // -----------------------------------------------------------------------

    /// Mark `[start, start+len)` used or free, checking the previous state.
    pub(crate) fn mark_range(&mut self, start: u32, len: u32, used: bool) -> Result<(), FsError> {
        if len == 0 {
            return Ok(());
        }
        let end = start
            .checked_add(len)
            .ok_or(FsError::Corrupt("block range overflow"))?;
        if start < self.geo.data_start || end > self.geo.backup_sb() {
            return Err(FsError::Corrupt("block range outside the data region"));
        }
        for b in start..end {
            if bits::get(&self.bbits, b) == used {
                return Err(FsError::Corrupt("block allocated or freed twice"));
            }
        }
        for b in start..end {
            if used {
                bits::set(&mut self.bbits, b);
            } else {
                bits::clear(&mut self.bbits, b);
            }
        }
        for i in start / BITS_PER_BITMAP_BLOCK..=(end - 1) / BITS_PER_BITMAP_BLOCK {
            self.tx.touched_bb.insert(i);
        }
        self.tx.blog.push((start, len, used));
        if used {
            self.free_blocks = self.free_blocks.saturating_sub(len);
        } else {
            self.free_blocks = self.free_blocks.saturating_add(len);
        }
        Ok(())
    }

    /// Allocate `want` blocks, preferring one contiguous run near `hint`;
    /// otherwise the largest available pieces. Returns `(start, len)` runs in
    /// allocation order.
    pub(crate) fn alloc_blocks(
        &mut self,
        want: u32,
        hint: u32,
    ) -> Result<Vec<(u32, u32)>, FsError> {
        if want == 0 {
            return Ok(Vec::new());
        }
        if want > self.free_blocks {
            return Err(FsError::NoSpace);
        }
        let lo = self.geo.data_start;
        let hi = self.geo.backup_sb();
        let hint = if hint < lo || hint >= hi { lo } else { hint };
        let start = bits::find_run(&self.bbits, hint, hi, want)
            .or_else(|| bits::find_run(&self.bbits, lo, hi, want));
        if let Some(s) = start {
            self.mark_range(s, want, true)?;
            self.alloc_hint = s + want;
            return Ok(alloc::vec![(s, want)]);
        }
        // Fragmented: take free runs from the hint onwards, then wrap around.
        let mut runs: Vec<(u32, u32)> = Vec::new();
        let mut remaining = want;
        for (from, to) in [(hint, hi), (lo, hint)] {
            let mut p = from;
            while remaining > 0 {
                let Some(s) = bits::next_free(&self.bbits, p, to) else {
                    break;
                };
                let e = bits::next_used(&self.bbits, s, to).unwrap_or(to);
                let take = (e - s).min(remaining);
                runs.push((s, take));
                remaining -= take;
                p = s + take;
            }
        }
        if remaining > 0 {
            return Err(FsError::NoSpace);
        }
        for &(s, l) in &runs {
            self.mark_range(s, l, true)?;
        }
        if let Some(&(s, l)) = runs.last() {
            self.alloc_hint = s + l;
        }
        Ok(runs)
    }

    // -----------------------------------------------------------------------
    // Transactions
    // -----------------------------------------------------------------------
}
