use bytes::{BufMut, BytesMut};
use frame::{Error, Frame, FrameSize, Head, Kind, StreamId};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct Settings {
    flags: SettingsFlags,
    fields: BTreeMap<u16, u32>,
}

/// An enum that lists all valid settings that can be sent in a SETTINGS
/// frame.
///
/// Each setting has a value that is a 32 bit unsigned integer (6.5.1.).
#[derive(Debug)]
pub enum Setting {
    HeaderTableSize(u32),
    EnablePush(u32),
    MaxConcurrentStreams(u32),
    InitialWindowSize(u32),
    MaxFrameSize(u32),
    MaxHeaderListSize(u32),
    Opaque(u16, u32),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub struct SettingsFlags(u8);

const ACK: u8 = 0x1;
const ALL: u8 = ACK;

/// The default value of SETTINGS_HEADER_TABLE_SIZE
pub const DEFAULT_SETTINGS_HEADER_TABLE_SIZE: usize = 4_096;

/// The default value of SETTINGS_INITIAL_WINDOW_SIZE
pub const DEFAULT_INITIAL_WINDOW_SIZE: u32 = 65_535;

/// The default value of MAX_FRAME_SIZE
pub const DEFAULT_MAX_FRAME_SIZE: FrameSize = 16_384;

/// INITIAL_WINDOW_SIZE upper bound
pub const MAX_INITIAL_WINDOW_SIZE: usize = (1 << 31) - 1;

/// MAX_FRAME_SIZE upper bound
pub const MAX_MAX_FRAME_SIZE: FrameSize = (1 << 24) - 1;

// ===== impl Settings =====

impl Settings {
    pub fn ack() -> Settings {
        Settings {
            flags: SettingsFlags::ack(),
            ..Settings::default()
        }
    }

    pub fn is_ack(&self) -> bool {
        self.flags.is_ack()
    }

    pub fn initial_window_size(&self) -> Option<u32> {
        self.get(Setting::INITIAL_WINDOW_SIZE)
    }

    pub fn set_initial_window_size(&mut self, size: Option<u32>) {
        self.set(Setting::INITIAL_WINDOW_SIZE, size);
    }

    pub fn max_concurrent_streams(&self) -> Option<u32> {
        self.get(Setting::MAX_CONCURRENT_STREAMS)
    }

    pub fn set_max_concurrent_streams(&mut self, max: Option<u32>) {
        self.set(Setting::MAX_CONCURRENT_STREAMS, max);
    }

    pub fn max_frame_size(&self) -> Option<u32> {
        self.get(Setting::MAX_FRAME_SIZE)
    }

    pub fn set_max_frame_size(&mut self, size: Option<u32>) {
        if let Some(val) = size {
            assert!(DEFAULT_MAX_FRAME_SIZE <= val && val <= MAX_MAX_FRAME_SIZE);
        }
        self.set(Setting::MAX_FRAME_SIZE, size);
    }

    pub fn max_header_list_size(&self) -> Option<u32> {
        self.get(Setting::MAX_HEADER_LIST_SIZE)
    }

    pub fn set_max_header_list_size(&mut self, size: Option<u32>) {
        self.set(Setting::MAX_HEADER_LIST_SIZE, size);
    }

    pub fn is_push_enabled(&self) -> bool {
        self.get(Setting::ENABLE_PUSH).unwrap_or(1) != 0
    }

    pub fn set_enable_push(&mut self, enable: bool) {
        self.set(Setting::ENABLE_PUSH, Some(enable as u32));
    }

    pub fn get(&self, id: u16) -> Option<u32> {
        self.fields.get(&id).map(|val| *val)
    }

    pub fn set(&mut self, id: u16, val: Option<u32>) {
        if let Some(val) = val {
            self.fields.insert(id, val);
        } else {
            self.fields.remove(&id);
        }
    }

    pub fn load(head: Head, payload: &[u8]) -> Result<Settings, Error> {
        use self::Setting::*;

        debug_assert_eq!(head.kind(), ::frame::Kind::Settings);

        if !head.stream_id().is_zero() {
            return Err(Error::InvalidStreamId);
        }

        // Load the flag
        let flag = SettingsFlags::load(head.flag());

        if flag.is_ack() {
            // Ensure that the payload is empty
            if payload.len() > 0 {
                return Err(Error::InvalidPayloadLength);
            }

            // Return the ACK frame
            return Ok(Settings::ack());
        }

        // Ensure the payload length is correct, each setting is 6 bytes long.
        if payload.len() % 6 != 0 {
            debug!("invalid settings payload length; len={:?}", payload.len());
            return Err(Error::InvalidPayloadAckSettings);
        }

        let mut settings = Settings::default();
        debug_assert!(!settings.flags.is_ack());

        for raw in payload.chunks(6) {
            match Setting::load(raw) {
                Some(HeaderTableSize(val)) => {
                    settings.set(Setting::HEADER_TABLE_SIZE, Some(val));
                },
                Some(EnablePush(val)) => match val {
                    0 | 1 => {
                        settings.set(Setting::ENABLE_PUSH, Some(val));
                    },
                    _ => {
                        return Err(Error::InvalidSettingValue);
                    },
                },
                Some(MaxConcurrentStreams(val)) => {
                    settings.set(Setting::MAX_CONCURRENT_STREAMS, Some(val));
                },
                Some(InitialWindowSize(val)) => if val as usize > MAX_INITIAL_WINDOW_SIZE {
                    return Err(Error::InvalidSettingValue);
                } else {
                    settings.set(Setting::INITIAL_WINDOW_SIZE, Some(val));
                },
                Some(MaxFrameSize(val)) => {
                    if val < DEFAULT_MAX_FRAME_SIZE || val > MAX_MAX_FRAME_SIZE {
                        return Err(Error::InvalidSettingValue);
                    } else {
                        settings.set(Setting::MAX_FRAME_SIZE, Some(val));
                    }
                },
                Some(MaxHeaderListSize(val)) => {
                    settings.set(Setting::MAX_HEADER_LIST_SIZE, Some(val));
                },
                Some(Opaque(id, val)) => {
                    settings.set(id, Some(val));
                }
                None => {},
            }
        }

        Ok(settings)
    }

    fn payload_len(&self) -> usize {
        let mut len = 0;
        self.for_each(|_| len += 6);
        len
    }

    pub fn encode(&self, dst: &mut BytesMut) {
        // Create & encode an appropriate frame head
        let head = Head::new(Kind::Settings, self.flags.into(), StreamId::zero());
        let payload_len = self.payload_len();

        trace!("encoding SETTINGS; len={}", payload_len);

        head.encode(payload_len, dst);

        // Encode the settings
        self.for_each(|setting| {
            trace!("encoding setting; val={:?}", setting);
            setting.encode(dst)
        });
    }

    fn for_each<F: FnMut(Setting)>(&self, mut f: F) {
        use self::Setting::*;

        for (&id, &val) in &self.fields {
            f(Opaque(id, val));
        }
    }
}

impl<T> From<Settings> for Frame<T> {
    fn from(src: Settings) -> Frame<T> {
        Frame::Settings(src)
    }
}

// ===== impl Setting =====

impl Setting {
    // Defined parameter identifiers (from section 6.5.2)
    const HEADER_TABLE_SIZE: u16 = 0x1;
    const ENABLE_PUSH: u16 = 0x2;
    const MAX_CONCURRENT_STREAMS: u16 = 0x3;
    const INITIAL_WINDOW_SIZE: u16 = 0x4;
    const MAX_FRAME_SIZE: u16 = 0x5;
    const MAX_HEADER_LIST_SIZE: u16 = 0x6;

    /// Creates a new `Setting` with the correct variant corresponding to the
    /// given setting id, based on the settings IDs defined in section
    /// 6.5.2.
    pub fn from_id(id: u16, val: u32) -> Option<Setting> {
        use self::Setting::*;

        match id {
            Setting::HEADER_TABLE_SIZE => Some(HeaderTableSize(val)),
            Setting::ENABLE_PUSH => Some(EnablePush(val)),
            Setting::MAX_CONCURRENT_STREAMS => Some(MaxConcurrentStreams(val)),
            Setting::INITIAL_WINDOW_SIZE => Some(InitialWindowSize(val)),
            Setting::MAX_FRAME_SIZE => Some(MaxFrameSize(val)),
            Setting::MAX_HEADER_LIST_SIZE => Some(MaxHeaderListSize(val)),
            id => Some(Opaque(id, val)),
        }
    }

    /// Creates a new `Setting` by parsing the given buffer of 6 bytes, which
    /// contains the raw byte representation of the setting, according to the
    /// "SETTINGS format" defined in section 6.5.1.
    ///
    /// The `raw` parameter should have length at least 6 bytes, since the
    /// length of the raw setting is exactly 6 bytes.
    ///
    /// # Panics
    ///
    /// If given a buffer shorter than 6 bytes, the function will panic.
    fn load(raw: &[u8]) -> Option<Setting> {
        let id: u16 = ((raw[0] as u16) << 8) | (raw[1] as u16);
        let val: u32 = unpack_octets_4!(raw, 2, u32);

        Setting::from_id(id, val)
    }

    fn encode(&self, dst: &mut BytesMut) {
        use self::Setting::*;

        let (kind, val) = match *self {
            HeaderTableSize(v) => (Setting::HEADER_TABLE_SIZE, v),
            EnablePush(v) => (Setting::ENABLE_PUSH, v),
            MaxConcurrentStreams(v) => (Setting::MAX_CONCURRENT_STREAMS, v),
            InitialWindowSize(v) => (Setting::INITIAL_WINDOW_SIZE, v),
            MaxFrameSize(v) => (Setting::MAX_FRAME_SIZE, v),
            MaxHeaderListSize(v) => (Setting::MAX_HEADER_LIST_SIZE, v),
            Opaque(i, v) => (i, v),
        };

        dst.put_u16_be(kind);
        dst.put_u32_be(val);
    }
}

// ===== impl SettingsFlags =====

impl SettingsFlags {
    pub fn empty() -> SettingsFlags {
        SettingsFlags(0)
    }

    pub fn load(bits: u8) -> SettingsFlags {
        SettingsFlags(bits & ALL)
    }

    pub fn ack() -> SettingsFlags {
        SettingsFlags(ACK)
    }

    pub fn is_ack(&self) -> bool {
        self.0 & ACK == ACK
    }
}

impl From<SettingsFlags> for u8 {
    fn from(src: SettingsFlags) -> u8 {
        src.0
    }
}
