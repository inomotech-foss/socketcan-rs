// socketcan/src/frame.rs
//
// Implements frames for CANbus 2.0 and FD for SocketCAN on Linux.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.


use crate::err::{CanError, CanErrorDecodingFailure, ConstructionError};
use crate::util::hal_id_to_raw;
use embedded_hal::can::{ExtendedId, Frame as EmbeddedFrame, Id, StandardId};
use libc::{can_frame, canfd_frame, canid_t};

use std::{convert::TryFrom, fmt, mem};

use itertools::Itertools;

/// if set, indicate 29 bit extended format
pub const EFF_FLAG: u32 = 0x80000000;

/// remote transmission request flag
pub const RTR_FLAG: u32 = 0x40000000;

/// error flag
pub const ERR_FLAG: u32 = 0x20000000;

/// valid bits in standard frame id
pub const SFF_MASK: u32 = 0x000007ff;

/// valid bits in extended frame id
pub const EFF_MASK: u32 = 0x1fffffff;

/// valid bits in error frame
pub const ERR_MASK: u32 = 0x1fffffff;

/// an error mask that will cause SocketCAN to report all errors
pub const ERR_MASK_ALL: u32 = ERR_MASK;

/// an error mask that will cause SocketCAN to silently drop all errors
pub const ERR_MASK_NONE: u32 = 0;

/// 'legacy' CAN frame
pub const CAN_DATA_LEN_MAX: usize = 8;

/// CAN FD frame
pub const CANFD_DATA_LEN_MAX: usize = 64;

/// CAN FD flags
pub const CANFD_BRS: u8 = 0x01; /* bit rate switch (second bitrate for payload data) */
pub const CANFD_ESI: u8 = 0x02; /* error state indicator of the transmitting node */

/// Creates a composite 32-bit CAN ID word for SocketCAN.
///
/// The ID 'word' is composed of the CAN ID along with the EFF/RTR/ERR bit flags.
fn init_id_word(id: canid_t, ext_id: bool, rtr: bool, err: bool) -> Result<canid_t, ConstructionError> {
    let mut _id = id;

    if id > EFF_MASK {
        return Err(ConstructionError::IDTooLarge);
    }

    if ext_id || id > SFF_MASK {
        _id |= EFF_FLAG;
    }

    if rtr {
        _id |= RTR_FLAG;
    }

    if err {
        _id |= ERR_FLAG;
    }

    Ok(_id)
}

fn is_extended(id: &Id) -> bool {
    match id {
        Id::Standard(_) => false,
        Id::Extended(_) => true,
    }
}

fn slice_to_array<const S: usize>(data: &[u8]) -> [u8; S] {
    let mut arr = [0; S];
    for (i, b) in data.iter().enumerate() {
        arr[i] = *b;
    }
    arr
}

// ===== Frame trait =====

pub trait Frame: EmbeddedFrame {
    /// Get the full SocketCAN ID word (with EFF/RTR/ERR flags)
    fn id_word(&self) -> u32;

    /// Return the actual raw CAN ID (without EFF/RTR/ERR flags)
    fn raw_id(&self) -> u32 {
        // TODO: Standard use SFF mask, or is this OK?
        self.id_word() & EFF_MASK
    }

    /// Return the CAN ID as the embedded HAL Id type.
    fn hal_id(&self) -> Id {
        if self.is_extended() {
            Id::Extended(ExtendedId::new(self.id_word() & EFF_MASK).unwrap())
        } else {
            Id::Standard(StandardId::new((self.id_word() & SFF_MASK) as u16).unwrap())
        }
    }

    /// Get the data length
    fn len(&self) -> usize {
        self.dlc()
    }

    /// Return the error message
    fn err(&self) -> u32 {
        self.id_word() & ERR_MASK
    }

    /// Check if frame is an error message
    fn is_error(&self) -> bool {
        self.id_word() & ERR_FLAG != 0
    }

    fn error(&self) -> Result<CanError, CanErrorDecodingFailure>
    where
        Self: Sized,
    {
        CanError::from_frame(self)
    }
}

// ===== CanAnyFrame =====

/// Any frame type.
pub enum CanAnyFrame {
    /// A classic CAN 2.0 frame, with up to 8-bytes of data
    Normal(CanFrame),
    /// A flexible data rate frame, with up to 64-bytes of data
    Fd(CanFdFrame),
}

impl fmt::Debug for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Normal(frame) => {
                write!(f, "CAN Frame {:?}", frame)
            }

            Self::Fd(frame) => {
                write!(f, "CAN FD Frame {:?}", frame)
            }
        }
    }
}

impl fmt::UpperHex for CanAnyFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Normal(frame) => frame.fmt(f),
            Self::Fd(frame) => frame.fmt(f),
        }
    }
}

impl From<CanFrame> for CanAnyFrame {
    fn from(frame: CanFrame) -> Self {
        Self::Normal(frame)
    }
}

impl From<CanFdFrame> for CanAnyFrame {
    fn from(frame: CanFdFrame) -> Self {
        Self::Fd(frame)
    }
}

// ===== CanFrame =====

/// The classic CAN 2.0 frame with up to 8-bytes of data.
///
/// This is highly compatible with the `can_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.can_frame.html))
#[derive(Clone, Copy)]
pub struct CanFrame(can_frame);

impl CanFrame {
    /// Initializes a CAN frame from raw parts.
    pub fn init(
        id: u32,
        data: &[u8],
        ext_id: bool,
        rtr: bool,
        err: bool,
    ) -> Result<Self, ConstructionError> {
        let n = data.len();

        if n > CAN_DATA_LEN_MAX {
            return Err(ConstructionError::TooMuchData);
        }

        let mut frame: can_frame = unsafe { mem::zeroed() };
        frame.can_id = init_id_word(id, ext_id, rtr, err)?;
        frame.can_dlc = n as u8;
        (&mut frame.data[..n]).copy_from_slice(data);

        Ok(Self(frame))
    }

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    pub fn as_ptr(&self) -> *const can_frame {
        &self.0 as *const can_frame
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    pub fn as_mut_ptr(&mut self) -> *mut can_frame {
        &mut self.0 as *mut can_frame
    }
}

impl EmbeddedFrame for CanFrame {
    /// Create a new frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let id = id.into();
        let is_ext = is_extended(&id);
        let raw_id = hal_id_to_raw(id);
        Self::init(raw_id, data, is_ext, false, false).ok()
    }

    /// Create a new remote transmission request frame.
    fn new_remote(id: impl Into<Id>, dlc: usize) -> Option<Self> {
        let id = id.into();
        let is_ext = is_extended(&id);
        let raw_id = hal_id_to_raw(id);
        let data = [0u8; 8];
        Self::init(raw_id, &data[0..dlc], is_ext, true, false).ok()
    }

    /// Check if frame uses 29 bit extended frame format
    fn is_extended(&self) -> bool {
        self.0.can_id & EFF_FLAG != 0
    }

    /// Check if frame is a remote transmission request.
    fn is_remote_frame(&self) -> bool {
        self.0.can_id & RTR_FLAG != 0
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    /// TODO: Return the proper DLC code for remote frames?
    fn dlc(&self) -> usize {
        self.0.can_dlc as usize
    }

    /// A slice into the actual data. Slice will always be <= 8 bytes in length
    fn data(&self) -> &[u8] {
        &self.0.data[..(self.0.can_dlc as usize)]
    }
}

impl Frame for CanFrame {
    fn id_word(&self) -> u32 {
        self.0.can_id
    }
}

impl Default for CanFrame {
    fn default() -> Self {
        let frame: can_frame = unsafe { mem::zeroed() };
        Self(frame)
    }
}

impl fmt::Debug for CanFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let _ = write!(f, "CanFrame {{ ")?;
        let _ = fmt::UpperHex::fmt(self, f)?;
        write!(f, " }}")
    }
}

impl fmt::UpperHex for CanFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self.0.can_id, "#")?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}

impl TryFrom<CanFdFrame> for CanFrame {
    type Error = ConstructionError;

    fn try_from(frame: CanFdFrame) -> Result<Self, Self::Error> {
        if frame.0.len > CAN_DATA_LEN_MAX as u8 {
            return Err(ConstructionError::TooMuchData);
        }

        CanFrame::init(
            frame.raw_id(),
            &frame.data()[..(frame.0.len as usize)],
            frame.is_extended(),
            false,
            frame.is_error(),
        )
    }
}

impl AsRef<libc::can_frame> for CanFrame {
    fn as_ref(&self) -> &can_frame {
            &self.0
    }
}

// ===== CanFdFrame =====

/// The CAN flexible data rate frame with up to 64-bytes of data.
///
/// This is highly compatible with the `canfd_frame` from libc.
/// ([ref](https://docs.rs/libc/latest/libc/struct.canfd_frame.html))
#[derive(Clone, Copy)]
pub struct CanFdFrame(canfd_frame);

impl CanFdFrame {
    pub fn init(
        id: u32,
        data: &[u8],
        ext_id: bool,
        err: bool,
        brs: bool,
        esi: bool,
    ) -> Result<Self, ConstructionError> {
        let n = data.len();

        if n > CAN_DATA_LEN_MAX {
            return Err(ConstructionError::TooMuchData);
        }

        let mut frame = Self::default();

        frame.0.can_id = init_id_word(id, ext_id, false, err)?;
        frame.0.len = n as u8;

        if brs {
            frame.0.flags |= CANFD_BRS;
        }
        if esi {
            frame.0.flags = CANFD_ESI;
        }

        (&mut frame.0.data[..n]).copy_from_slice(data);

        Ok(frame)
    }

    pub fn is_brs(&self) -> bool {
        self.0.flags & CANFD_BRS == CANFD_BRS
    }

    pub fn set_brs(&mut self, on: bool) {
        if on {
            self.0.flags |= CANFD_BRS;
        } else {
            self.0.flags &= !CANFD_BRS;
        }
    }

    pub fn is_esi(&self) -> bool {
        self.0.flags & CANFD_ESI == CANFD_ESI
    }

    pub fn set_esi(&mut self, on: bool) {
        if on {
            self.0.flags |= CANFD_ESI;
        } else {
            self.0.flags &= !CANFD_ESI;
        }
    }

    /// Gets a pointer to the CAN frame structure that is compatible with
    /// the Linux C API.
    pub fn as_ptr(&self) -> *const canfd_frame {
        &self.0 as *const canfd_frame
    }

    /// Gets a mutable pointer to the CAN frame structure that is compatible
    /// with the Linux C API.
    pub fn as_mut_ptr(&mut self) -> *mut canfd_frame {
        &mut self.0 as *mut canfd_frame
    }
}

impl EmbeddedFrame for CanFdFrame {
    /// Create a new frame
    fn new(id: impl Into<Id>, data: &[u8]) -> Option<Self> {
        let id = id.into();
        let is_ext = is_extended(&id);
        let raw_id = hal_id_to_raw(id);
        Self::init(raw_id, data, is_ext, false, false, false).ok()
    }

    /// CAN FD frames don't support remote
    fn new_remote(_id: impl Into<Id>, _dlc: usize) -> Option<Self> {
        None
    }

    /// Check if frame uses 29 bit extended frame format
    fn is_extended(&self) -> bool {
        self.0.can_id & EFF_FLAG != 0
    }

    /// The FD frames don't support remote request
    fn is_remote_frame(&self) -> bool {
        false
    }

    /// Return the frame identifier.
    fn id(&self) -> Id {
        self.hal_id()
    }

    /// Data length
    fn dlc(&self) -> usize {
        self.0.len as usize
    }

    /// A slice into the actual data.
    ///
    /// For normal CAN frames the slice will always be <= 8 bytes in length.
    fn data(&self) -> &[u8] {
        &self.0.data[..(self.0.len as usize)]
    }
}

impl Frame for CanFdFrame {
    fn id_word(&self) -> u32 {
        self.0.can_id
    }
}

impl Default for CanFdFrame {
    fn default() -> Self {
        let frame: canfd_frame = unsafe { mem::zeroed() };
        Self(frame)
    }
}

impl fmt::Debug for CanFdFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let _ = write!(f, "CanFdFrame {{ ")?;
        let _ = fmt::UpperHex::fmt(self, f)?;
        write!(f, " }}")
    }
}

impl fmt::UpperHex for CanFdFrame {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{:X}{}", self.0.can_id, "##")?;
        write!(f, "{} ", self.0.flags)?;
        let mut parts = self.data().iter().map(|v| format!("{:02X}", v));
        let sep = if f.alternate() { " " } else { " " };
        write!(f, "{}", parts.join(sep))
    }
}

impl From<CanFrame> for CanFdFrame {
    fn from(frame: CanFrame) -> Self {
        let mut fdframe = Self::default();
        // TODO: force rtr off?
        fdframe.0.can_id = frame.0.can_id;
        fdframe.0.len = frame.0.can_dlc as u8;
        fdframe.0.data = slice_to_array::<CANFD_DATA_LEN_MAX>(frame.data());
        fdframe
    }
}

impl AsRef<libc::canfd_frame> for CanFdFrame {
    fn as_ref(&self) -> &canfd_frame {
            &self.0
    }
}


