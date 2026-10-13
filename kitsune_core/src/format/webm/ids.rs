//! The element ids this demuxer reads (Matroska specification, with the length marker bits).
//! Segment children not listed (`SeekHead`, `Tags`, `Chapters`, `Attachments`...) are skipped.

pub(super) const EBML: u32 = 0x1A45_DFA3;
pub(super) const DOC_TYPE: u32 = 0x4282;
pub(super) const SEGMENT: u32 = 0x1853_8067;

pub(super) const INFO: u32 = 0x1549_A966;
pub(super) const TRACKS: u32 = 0x1654_AE6B;
pub(super) const CLUSTER: u32 = 0x1F43_B675;
pub(super) const CUES: u32 = 0x1C53_BB6B;
pub(super) const VOID: u32 = 0xEC;
pub(super) const CRC32: u32 = 0xBF;

pub(super) const TIMECODE_SCALE: u32 = 0x2A_D7B1;
pub(super) const DURATION: u32 = 0x4489;

pub(super) const TRACK_ENTRY: u32 = 0xAE;
pub(super) const TRACK_NUMBER: u32 = 0xD7;
pub(super) const TRACK_UID: u32 = 0x73C5;
pub(super) const TRACK_TYPE: u32 = 0x83;
pub(super) const CODEC_ID: u32 = 0x86;
pub(super) const CODEC_PRIVATE: u32 = 0x63A2;
pub(super) const DEFAULT_DURATION: u32 = 0x23_E383;
pub(super) const CODEC_DELAY: u32 = 0x56AA;
pub(super) const SEEK_PREROLL: u32 = 0x56BB;
pub(super) const VIDEO: u32 = 0xE0;
pub(super) const PIXEL_WIDTH: u32 = 0xB0;
pub(super) const PIXEL_HEIGHT: u32 = 0xBA;
pub(super) const AUDIO: u32 = 0xE1;
pub(super) const SAMPLING_FREQUENCY: u32 = 0xB5;
pub(super) const CHANNELS: u32 = 0x9F;
pub(super) const CONTENT_ENCODINGS: u32 = 0x6D80;

pub(super) const TIMECODE: u32 = 0xE7;
pub(super) const POSITION: u32 = 0xA7;
pub(super) const PREV_SIZE: u32 = 0xAB;
pub(super) const SILENT_TRACKS: u32 = 0x5854;
pub(super) const SIMPLE_BLOCK: u32 = 0xA3;
pub(super) const BLOCK_GROUP: u32 = 0xA0;
pub(super) const ENCRYPTED_BLOCK: u32 = 0xAF;
pub(super) const BLOCK: u32 = 0xA1;
pub(super) const BLOCK_DURATION: u32 = 0x9B;
pub(super) const REFERENCE_BLOCK: u32 = 0xFB;
pub(super) const DISCARD_PADDING: u32 = 0x75A2;

pub(super) const CUE_POINT: u32 = 0xBB;
pub(super) const CUE_TIME: u32 = 0xB3;
pub(super) const CUE_TRACK_POSITIONS: u32 = 0xB7;
pub(super) const CUE_TRACK: u32 = 0xF7;
pub(super) const CUE_CLUSTER_POSITION: u32 = 0xF1;

/// The ids that may appear directly in a cluster: anything else ends a cluster of unknown size.
pub(super) fn is_cluster_child(id: u32) -> bool {
    matches!(
        id,
        TIMECODE
            | POSITION
            | PREV_SIZE
            | SILENT_TRACKS
            | SIMPLE_BLOCK
            | BLOCK_GROUP
            | ENCRYPTED_BLOCK
            | VOID
            | CRC32
    )
}

/// Ids that are never children of a segment: one of them ends a segment of unknown size.
pub(super) fn ends_segment(id: u32) -> bool {
    matches!(id, EBML | SEGMENT)
}
