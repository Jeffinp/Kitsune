//! Metadata blocks: the checked cache, typed reads, writes of checksummed blocks and freeing them.

use super::*;

impl<D: BlockDevice> Fs3<D> {
    pub(crate) fn ready(&self) -> Result<(), FsError> {
        if self.poisoned {
            Err(FsError::Poisoned)
        } else {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // Block access through the transaction overlay
    // -----------------------------------------------------------------------

    pub(crate) fn is_data_block(&self, b: u32) -> bool {
        b >= self.geo.data_start && b < self.geo.backup_sb()
    }

    /// Read a metadata block, seeing this transaction's uncommitted changes.
    pub(crate) fn meta_get(&mut self, blk: u32) -> Result<&Block, FsError> {
        if let Some(b) = self.tx.meta.get(&blk) {
            return Ok(b);
        }
        Ok(self.cache.get(blk as u64)?)
    }

    /// Get a metadata block for modification (copied into the overlay).
    pub(crate) fn meta_mut(&mut self, blk: u32) -> Result<&mut Block, FsError> {
        if !self.tx.meta.contains_key(&blk) {
            if self.tx.meta.len() >= self.geo.journal_blocks as usize {
                return Err(FsError::TxTooLarge);
            }
            let mut b = Box::new([0u8; BLOCK_SIZE]);
            self.cache.read(blk as u64, &mut b)?;
            self.tx.meta.insert(blk, b);
        }
        self.tx
            .meta
            .get_mut(&blk)
            .map(|b| &mut **b)
            .ok_or(FsError::Corrupt("transaction overlay"))
    }

    /// Replace a metadata block wholesale (no read needed).
    pub(crate) fn put_meta(&mut self, blk: u32, img: Block) -> Result<(), FsError> {
        if !self.tx.meta.contains_key(&blk)
            && self.tx.meta.len() >= self.geo.journal_blocks as usize
        {
            return Err(FsError::TxTooLarge);
        }
        self.tx.meta.insert(blk, Box::new(img));
        Ok(())
    }

    /// Read a directory (`ext = false`) or extent (`ext = true`) block,
    /// verifying its checksum once per load into the cache.
    pub(crate) fn typed_block(
        &mut self,
        blk: u32,
        ext: bool,
        owner: u32,
    ) -> Result<&Block, FsError> {
        if !self.is_data_block(blk) {
            return Err(FsError::Corrupt("metadata block out of range"));
        }
        if let Some(b) = self.tx.meta.get(&blk) {
            return Ok(b);
        }
        let want = 1 + ext as u8;
        let verified = {
            let (b, tag) = self.cache.get_tagged(blk as u64)?;
            if tag == want {
                false
            } else {
                verify_typed(b, ext, owner)?;
                true
            }
        };
        if verified {
            self.cache.set_tag(blk as u64, want);
        }
        Ok(self.cache.get(blk as u64)?)
    }

    /// Free a metadata block (directory / extent block) at commit, dropping any
    /// pending change to it.
    pub(crate) fn free_meta_block(&mut self, blk: u32) {
        self.tx.meta.remove(&blk);
        self.tx.to_free.push((blk, 1));
    }
}
