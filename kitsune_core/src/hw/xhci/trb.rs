//! Transfer request blocks: the 16-byte records of the command, transfer and event rings.
//!
//! A TRB is `parameter` (64 bits), `status` (32) and `control` (32). Bit 0 of `control` is the cycle bit
//! (who owns the slot), bits 15:10 the TRB type; the rest depends on the type. Everything here builds or
//! decodes those words; writing them into DMA memory is the kernel's job.

pub const TRB_BYTES: usize = 16;

// TRB types (control bits 15:10).
pub const TYPE_NORMAL: u8 = 1;
pub const TYPE_SETUP_STAGE: u8 = 2;
pub const TYPE_DATA_STAGE: u8 = 3;
pub const TYPE_STATUS_STAGE: u8 = 4;
pub const TYPE_LINK: u8 = 6;
pub const TYPE_ENABLE_SLOT: u8 = 9;
pub const TYPE_DISABLE_SLOT: u8 = 10;
pub const TYPE_ADDRESS_DEVICE: u8 = 11;
pub const TYPE_CONFIGURE_ENDPOINT: u8 = 12;
pub const TYPE_EVALUATE_CONTEXT: u8 = 13;
pub const TYPE_RESET_ENDPOINT: u8 = 14;
pub const TYPE_NO_OP_COMMAND: u8 = 23;
pub const TYPE_TRANSFER_EVENT: u8 = 32;
pub const TYPE_COMMAND_COMPLETION: u8 = 33;
pub const TYPE_PORT_STATUS_CHANGE: u8 = 34;

const CYCLE: u32 = 1 << 0;
const TOGGLE_CYCLE: u32 = 1 << 1; // link TRB: flip the producer's cycle after following the link
const INTERRUPT_ON_SHORT: u32 = 1 << 2;
const INTERRUPT_ON_COMPLETION: u32 = 1 << 5;
const IMMEDIATE_DATA: u32 = 1 << 6;
const BLOCK_SET_ADDRESS: u32 = 1 << 9;
const DIRECTION_IN: u32 = 1 << 16;

/// One TRB.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Trb {
    pub param: u64,
    pub status: u32,
    pub control: u32,
}

impl Trb {
    pub const fn new(param: u64, status: u32, control: u32) -> Self {
        Self {
            param,
            status,
            control,
        }
    }

    /// Reassembles a TRB from the four little-endian dwords it occupies in memory.
    pub const fn from_dwords(d: [u32; 4]) -> Self {
        Self {
            param: d[0] as u64 | (d[1] as u64) << 32,
            status: d[2],
            control: d[3],
        }
    }

    pub const fn to_dwords(&self) -> [u32; 4] {
        [
            self.param as u32,
            (self.param >> 32) as u32,
            self.status,
            self.control,
        ]
    }

    pub const fn trb_type(&self) -> u8 {
        ((self.control >> 10) & 0x3F) as u8
    }

    pub const fn cycle(&self) -> bool {
        self.control & CYCLE != 0
    }

    /// The same TRB with its cycle bit set to `cycle`.
    pub const fn with_cycle(mut self, cycle: bool) -> Self {
        self.control = (self.control & !CYCLE) | cycle as u32;
        self
    }

    const fn typed(trb_type: u8) -> u32 {
        (trb_type as u32) << 10
    }

    // ---- ring linkage ----

    /// Link TRB: the ring continues at `next` (16-byte aligned). With `toggle_cycle` the producer flips
    /// its cycle state after the link, which is how the last slot of a ring closes the loop.
    pub const fn link(next: u64, toggle_cycle: bool) -> Self {
        let toggle = if toggle_cycle { TOGGLE_CYCLE } else { 0 };
        Self::new(next & !0xF, 0, Self::typed(TYPE_LINK) | toggle)
    }

    // ---- commands ----

    pub const fn no_op_command() -> Self {
        Self::new(0, 0, Self::typed(TYPE_NO_OP_COMMAND))
    }

    pub const fn enable_slot() -> Self {
        Self::new(0, 0, Self::typed(TYPE_ENABLE_SLOT))
    }

    pub const fn disable_slot(slot: u8) -> Self {
        Self::new(0, 0, Self::typed(TYPE_DISABLE_SLOT) | (slot as u32) << 24)
    }

    /// Address Device: `input_context` is the physical address of the input context (16-byte aligned).
    /// With `block_set_address` the controller prepares the slot but does not send SET_ADDRESS.
    pub const fn address_device(input_context: u64, slot: u8, block_set_address: bool) -> Self {
        let bsr = if block_set_address {
            BLOCK_SET_ADDRESS
        } else {
            0
        };
        Self::new(
            input_context & !0xF,
            0,
            Self::typed(TYPE_ADDRESS_DEVICE) | bsr | (slot as u32) << 24,
        )
    }

    pub const fn configure_endpoint(input_context: u64, slot: u8) -> Self {
        Self::new(
            input_context & !0xF,
            0,
            Self::typed(TYPE_CONFIGURE_ENDPOINT) | (slot as u32) << 24,
        )
    }

    pub const fn evaluate_context(input_context: u64, slot: u8) -> Self {
        Self::new(
            input_context & !0xF,
            0,
            Self::typed(TYPE_EVALUATE_CONTEXT) | (slot as u32) << 24,
        )
    }

    /// Reset Endpoint after a stall. `dci` is the device context index of the endpoint.
    pub const fn reset_endpoint(slot: u8, dci: u8) -> Self {
        Self::new(
            0,
            0,
            Self::typed(TYPE_RESET_ENDPOINT) | (dci as u32 & 0x1F) << 16 | (slot as u32) << 24,
        )
    }

    // ---- transfers ----

    /// Setup stage of a control transfer; the eight setup bytes travel inside the TRB.
    pub const fn setup_stage(setup: SetupPacket, data: DataStage) -> Self {
        let transfer_type = match data {
            DataStage::None => 0,
            DataStage::Out => 2,
            DataStage::In => 3,
        };
        Self::new(
            setup.pack(),
            8,
            Self::typed(TYPE_SETUP_STAGE) | IMMEDIATE_DATA | transfer_type << 16,
        )
    }

    /// Data stage of a control transfer: `len` bytes at physical address `buffer`.
    pub const fn data_stage(buffer: u64, len: u16, data_in: bool) -> Self {
        let dir = if data_in { DIRECTION_IN } else { 0 };
        Self::new(buffer, len as u32, Self::typed(TYPE_DATA_STAGE) | dir)
    }

    /// Status stage, which completes the transfer and reports it (interrupt on completion).
    pub const fn status_stage(data_in: bool) -> Self {
        let dir = if data_in { DIRECTION_IN } else { 0 };
        Self::new(
            0,
            0,
            Self::typed(TYPE_STATUS_STAGE) | dir | INTERRUPT_ON_COMPLETION,
        )
    }

    /// Normal TRB for an interrupt or bulk endpoint: `len` bytes at `buffer`, one event when done (also
    /// on a short packet, which is what an interrupt endpoint's report usually is).
    pub const fn normal(buffer: u64, len: u16) -> Self {
        Self::new(
            buffer,
            len as u32,
            Self::typed(TYPE_NORMAL) | INTERRUPT_ON_SHORT | INTERRUPT_ON_COMPLETION,
        )
    }
}

/// Direction of the optional data stage of a control transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataStage {
    None,
    Out,
    In,
}

/// The eight-byte SETUP packet of a control transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupPacket {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub length: u16,
}

pub const DESCRIPTOR_DEVICE: u8 = 1;
pub const DESCRIPTOR_CONFIGURATION: u8 = 2;

impl SetupPacket {
    /// The packet as the 64-bit parameter of a setup-stage TRB (little-endian field order).
    pub const fn pack(&self) -> u64 {
        self.request_type as u64
            | (self.request as u64) << 8
            | (self.value as u64) << 16
            | (self.index as u64) << 32
            | (self.length as u64) << 48
    }

    /// Device-to-host: the data stage reads.
    pub const fn is_in(&self) -> bool {
        self.request_type & 0x80 != 0
    }

    /// GET_DESCRIPTOR from the device.
    pub const fn get_descriptor(kind: u8, index: u8, length: u16) -> Self {
        Self {
            request_type: 0x80,
            request: 6,
            value: (kind as u16) << 8 | index as u16,
            index: 0,
            length,
        }
    }

    pub const fn set_configuration(configuration: u8) -> Self {
        Self {
            request_type: 0x00,
            request: 9,
            value: configuration as u16,
            index: 0,
            length: 0,
        }
    }

    /// HID SET_PROTOCOL for `interface`: `0` selects the boot protocol, `1` the report protocol.
    pub const fn hid_set_protocol(interface: u8, protocol: u8) -> Self {
        Self {
            request_type: 0x21,
            request: 0x0B,
            value: protocol as u16,
            index: interface as u16,
            length: 0,
        }
    }

    /// HID SET_IDLE: `duration` in units of 4 ms (`0` = report only when something changes).
    pub const fn hid_set_idle(interface: u8, duration: u8, report_id: u8) -> Self {
        Self {
            request_type: 0x21,
            request: 0x0A,
            value: (duration as u16) << 8 | report_id as u16,
            index: interface as u16,
            length: 0,
        }
    }
}

/// The TRBs of one control transfer, in ring order, and how many are used (2 or 3). The data stage reads
/// or writes `setup.length` bytes at `buffer`; with `length == 0` there is none. The status stage runs in
/// the opposite direction to the data (and `IN` when there is no data), as the USB protocol requires.
pub const fn control_transfer(setup: SetupPacket, buffer: u64) -> ([Trb; 3], usize) {
    if setup.length == 0 {
        return (
            [
                Trb::setup_stage(setup, DataStage::None),
                Trb::status_stage(true),
                Trb::new(0, 0, 0),
            ],
            2,
        );
    }
    let data_in = setup.is_in();
    let direction = if data_in {
        DataStage::In
    } else {
        DataStage::Out
    };
    (
        [
            Trb::setup_stage(setup, direction),
            Trb::data_stage(buffer, setup.length, data_in),
            Trb::status_stage(!data_in),
        ],
        3,
    )
}

// ---- events ----

/// Completion code of a command or transfer event (status bits 31:24).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionCode {
    Success,
    DataBufferError,
    BabbleDetected,
    UsbTransactionError,
    TrbError,
    StallError,
    ResourceError,
    BandwidthError,
    NoSlotsAvailable,
    ShortPacket,
    CommandRingStopped,
    CommandAborted,
    Other(u8),
}

impl CompletionCode {
    pub const fn from_u8(code: u8) -> Self {
        match code {
            1 => Self::Success,
            2 => Self::DataBufferError,
            3 => Self::BabbleDetected,
            4 => Self::UsbTransactionError,
            5 => Self::TrbError,
            6 => Self::StallError,
            7 => Self::ResourceError,
            8 => Self::BandwidthError,
            9 => Self::NoSlotsAvailable,
            13 => Self::ShortPacket,
            24 => Self::CommandRingStopped,
            25 => Self::CommandAborted,
            other => Self::Other(other),
        }
    }

    /// The operation worked: complete, or a short packet (fewer bytes than asked, not an error).
    pub const fn is_ok(self) -> bool {
        matches!(self, Self::Success | Self::ShortPacket)
    }
}

/// A decoded event-ring TRB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    /// A command finished. `slot` is the slot it created or acted on.
    CommandCompletion {
        command_trb: u64,
        code: CompletionCode,
        slot: u8,
    },
    /// A transfer TRB finished. `residual` is the number of bytes NOT transferred.
    Transfer {
        trb: u64,
        code: CompletionCode,
        slot: u8,
        endpoint: u8,
        residual: u32,
    },
    /// Something happened on root-hub port `port` (1-based): connect, disconnect, reset done.
    PortStatusChange { port: u8, code: CompletionCode },
    /// Any other event type; the driver ignores it.
    Other { trb_type: u8 },
}

impl Event {
    pub const fn decode(trb: &Trb) -> Self {
        let code = CompletionCode::from_u8((trb.status >> 24) as u8);
        let slot = (trb.control >> 24) as u8;
        match trb.trb_type() {
            TYPE_COMMAND_COMPLETION => Self::CommandCompletion {
                command_trb: trb.param,
                code,
                slot,
            },
            TYPE_TRANSFER_EVENT => Self::Transfer {
                trb: trb.param,
                code,
                slot,
                endpoint: ((trb.control >> 16) & 0x1F) as u8,
                residual: trb.status & 0x00FF_FFFF,
            },
            TYPE_PORT_STATUS_CHANGE => Self::PortStatusChange {
                port: (trb.param >> 24) as u8,
                code,
            },
            other => Self::Other { trb_type: other },
        }
    }
}
