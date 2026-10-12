//! Transactions: begin, abort, the commit sequence and the journal writes.

use super::*;

impl<D: BlockDevice> Fs3<D> {
    pub(in super::super) fn begin(&mut self) -> Result<(), FsError> {
        self.ready()?;
        self.tx = Tx::idle();
        self.tx.active = true;
        self.tx.free_blocks0 = self.free_blocks;
        self.tx.free_inodes0 = self.free_inodes;
        self.tx.hint0 = self.alloc_hint;
        Ok(())
    }

    /// Undo everything the transaction did in RAM (nothing reached the journal).
    pub(in super::super) fn abort(&mut self) {
        let tx = core::mem::replace(&mut self.tx, Tx::idle());
        for &(s, l, now) in tx.blog.iter().rev() {
            for b in s..s + l {
                if now {
                    bits::clear(&mut self.bbits, b);
                } else {
                    bits::set(&mut self.bbits, b);
                }
            }
        }
        for &(bit, now) in tx.ilog.iter().rev() {
            if now {
                bits::clear(&mut self.ibits, bit);
            } else {
                bits::set(&mut self.ibits, bit);
            }
        }
        self.free_blocks = tx.free_blocks0;
        self.free_inodes = tx.free_inodes0;
        self.alloc_hint = tx.hint0;
        // Data blocks written for the aborted transaction are garbage in free
        // blocks; make sure the cache never writes them back later.
        for &(s, l) in &tx.data {
            for b in s..s + l {
                self.cache.discard(b as u64);
            }
        }
    }

    /// Run `f` as one atomic, durable transaction.
    pub(crate) fn txn<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> Result<R, FsError>,
    ) -> Result<R, FsError> {
        self.begin()?;
        match f(self) {
            Ok(r) => {
                self.commit()?;
                Ok(r)
            }
            Err(e) => {
                self.abort();
                Err(e)
            }
        }
    }

    pub(in super::super) fn commit(&mut self) -> Result<(), FsError> {
        if let Err(e) = self.prepare_commit() {
            self.abort();
            return Err(e);
        }
        if self.tx.meta.is_empty() {
            self.tx = Tx::idle();
            return Ok(());
        }
        // From here on the transaction may be (partly) on the medium: any
        // failure leaves RAM and disk in an unknown relation, so poison.
        match self.write_journal() {
            Ok(()) => {
                self.commits += 1;
                self.tx = Tx::idle();
                Ok(())
            }
            Err(e) => {
                self.poisoned = true;
                self.tx = Tx::idle();
                Err(e)
            }
        }
    }

    /// Apply the deferred frees and turn the touched bitmap blocks into images.
    pub(in super::super) fn prepare_commit(&mut self) -> Result<(), FsError> {
        let frees = core::mem::take(&mut self.tx.to_free);
        for (s, l) in frees {
            self.mark_range(s, l, false)?;
        }
        let bb: Vec<u32> = self.tx.touched_bb.iter().copied().collect();
        for i in bb {
            let img = bitmap_image(&self.bbits, i);
            self.put_meta(self.geo.bbitmap_start + i, img)?;
        }
        let ib: Vec<u32> = self.tx.touched_ib.iter().copied().collect();
        for i in ib {
            let img = bitmap_image(&self.ibits, i);
            self.put_meta(self.geo.ibitmap_start + i, img)?;
        }
        if self.tx.meta.len() > self.geo.journal_blocks as usize {
            return Err(FsError::TxTooLarge);
        }
        Ok(())
    }

    pub(in super::super) fn write_journal(&mut self) -> Result<(), FsError> {
        let geo = self.geo;
        let targets: Vec<u32> = self.tx.meta.keys().copied().collect();
        let blocks: Vec<&Block> = self.tx.meta.values().map(|b| &**b).collect();
        // 1. Payload into the journal; the flush also makes the new data
        //    blocks (written copy-on-write by this transaction) durable.
        for (i, b) in blocks.iter().enumerate() {
            self.cache
                .write_direct((geo.jpayload + i as u32) as u64, b, 0)?;
        }
        self.cache.flush()?;
        // 2. Commit record. After this flush the transaction is committed.
        let seq = self.seq + 1;
        let hdr = build_journal_header(seq, &targets, &blocks);
        self.cache.write_direct(geo.jhdr as u64, &hdr, 0)?;
        self.cache.flush()?;
        // 3. Checkpoint to the home locations.
        for (t, b) in targets.iter().zip(blocks.iter()) {
            let tag = if b[4..8] == DIR_MAGIC {
                1
            } else if b[4..8] == EXT_MAGIC {
                2
            } else {
                0
            };
            self.cache.write_direct(*t as u64, b, tag)?;
        }
        self.cache.flush()?;
        // 4. Retire the record before the payload area is reused.
        let clear = build_journal_header(seq, &[], &[]);
        self.cache.write_direct(geo.jhdr as u64, &clear, 0)?;
        self.cache.flush()?;
        self.seq = seq;
        Ok(())
    }
}
