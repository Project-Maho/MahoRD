use crate::{
    codec::{checked_bytes, push_i32, push_u16, push_u32, usize_to_u16, Decoder},
    CodecError, WireCodec,
};

pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControlMessageType {
    RequestKeyFrame = 0,
    StartStream = 1,
    StopStream = 2,
    Disconnect = 3,
    Ping = 4,
    Pong = 5,
    BitrateAdjust = 6,
    StreamConfigRequest = 7,
    StreamConfigResponse = 8,
    StreamConfigReject = 9,
    StreamConfigError = 10,
    ClipboardSyncRequest = 11,
    ClipboardSyncUpdate = 12,
    ClipboardSyncError = 13,
    InputAck = 14,
}

impl ControlMessageType {
    pub const ALL: [Self; 15] = [
        Self::RequestKeyFrame,
        Self::StartStream,
        Self::StopStream,
        Self::Disconnect,
        Self::Ping,
        Self::Pong,
        Self::BitrateAdjust,
        Self::StreamConfigRequest,
        Self::StreamConfigResponse,
        Self::StreamConfigReject,
        Self::StreamConfigError,
        Self::ClipboardSyncRequest,
        Self::ClipboardSyncUpdate,
        Self::ClipboardSyncError,
        Self::InputAck,
    ];
}

impl TryFrom<u8> for ControlMessageType {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::RequestKeyFrame),
            1 => Ok(Self::StartStream),
            2 => Ok(Self::StopStream),
            3 => Ok(Self::Disconnect),
            4 => Ok(Self::Ping),
            5 => Ok(Self::Pong),
            6 => Ok(Self::BitrateAdjust),
            7 => Ok(Self::StreamConfigRequest),
            8 => Ok(Self::StreamConfigResponse),
            9 => Ok(Self::StreamConfigReject),
            10 => Ok(Self::StreamConfigError),
            11 => Ok(Self::ClipboardSyncRequest),
            12 => Ok(Self::ClipboardSyncUpdate),
            13 => Ok(Self::ClipboardSyncError),
            14 => Ok(Self::InputAck),
            unknown => Err(CodecError::UnknownControlMessageType(unknown)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitrateAdjust {
    pub target_bitrate: i32,
}

impl BitrateAdjust {
    pub const SIZE: usize = 4;
}

impl WireCodec for BitrateAdjust {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(Self::SIZE);
        push_i32(&mut output, self.target_bitrate);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let target_bitrate = decoder.i32("target bitrate")?;
        decoder.finish("bitrate adjustment")?;
        Ok(Self { target_bitrate })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputAckMessage {
    pub sequence: u32,
    pub success: bool,
    pub error_code: u8,
}

impl InputAckMessage {
    pub const SIZE: usize = 6;
}

impl WireCodec for InputAckMessage {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(Self::SIZE);
        push_u32(&mut output, self.sequence);
        output.push(self.success as u8);
        output.push(self.error_code);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let sequence = decoder.u32("input ack sequence")?;
        let success = decoder.u8("input ack success")? != 0;
        let error_code = decoder.u8("input ack error code")?;
        decoder.finish("input ack message")?;
        Ok(Self {
            sequence,
            success,
            error_code,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfiguration {
    pub width: u32,
    pub height: u32,
    pub bitrate: u32,
    pub frames_per_second: u16,
}

impl StreamConfiguration {
    pub const SIZE: usize = 14;

    fn encode_into(&self, output: &mut Vec<u8>) {
        push_u32(output, self.width);
        push_u32(output, self.height);
        push_u32(output, self.bitrate);
        push_u16(output, self.frames_per_second);
    }

    fn decode_from(decoder: &mut Decoder<'_>) -> Result<Self, CodecError> {
        Ok(Self {
            width: decoder.u32("stream width")?,
            height: decoder.u32("stream height")?,
            bitrate: decoder.u32("stream bitrate")?,
            frames_per_second: decoder.u16("stream frames per second")?,
        })
    }
}

impl WireCodec for StreamConfiguration {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(Self::SIZE);
        self.encode_into(&mut output);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let value = Self::decode_from(&mut decoder)?;
        decoder.finish("stream configuration")?;
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfigurationRequest {
    pub request_id: u32,
    pub desired: StreamConfiguration,
}

impl StreamConfigurationRequest {
    pub const SIZE: usize = 18;
}

impl WireCodec for StreamConfigurationRequest {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(Self::SIZE);
        push_u32(&mut output, self.request_id);
        self.desired.encode_into(&mut output);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let request_id = decoder.u32("stream request ID")?;
        let desired = StreamConfiguration::decode_from(&mut decoder)?;
        decoder.finish("stream configuration request")?;
        Ok(Self {
            request_id,
            desired,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfigurationResponse {
    pub request_id: u32,
    pub active: StreamConfiguration,
}

impl StreamConfigurationResponse {
    pub const SIZE: usize = 18;
}

impl WireCodec for StreamConfigurationResponse {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(Self::SIZE);
        push_u32(&mut output, self.request_id);
        self.active.encode_into(&mut output);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let request_id = decoder.u32("stream response ID")?;
        let active = StreamConfiguration::decode_from(&mut decoder)?;
        decoder.finish("stream configuration response")?;
        Ok(Self { request_id, active })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum StreamConfigurationErrorCode {
    InvalidRequest = 0,
    UnsupportedDimensions = 1,
    UnsupportedBitrate = 2,
    UnsupportedFps = 3,
    RejectedByPeer = 4,
}

impl TryFrom<u8> for StreamConfigurationErrorCode {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::InvalidRequest),
            1 => Ok(Self::UnsupportedDimensions),
            2 => Ok(Self::UnsupportedBitrate),
            3 => Ok(Self::UnsupportedFps),
            4 => Ok(Self::RejectedByPeer),
            unknown => Err(CodecError::UnknownStreamConfigurationErrorCode(unknown)),
        }
    }
}

fn encode_stream_problem(
    request_id: u32,
    code: StreamConfigurationErrorCode,
    message: &str,
) -> Result<Vec<u8>, CodecError> {
    let message = checked_bytes(message, "stream configuration message", u16::MAX as usize)?;
    let mut output = Vec::with_capacity(7 + message.len());
    push_u32(&mut output, request_id);
    output.push(code as u8);
    push_u16(
        &mut output,
        usize_to_u16(message.len(), "stream configuration message")?,
    );
    output.extend_from_slice(message);
    Ok(output)
}

fn decode_stream_problem(
    input: &[u8],
    field: &'static str,
) -> Result<(u32, StreamConfigurationErrorCode, String), CodecError> {
    let mut decoder = Decoder::new(input);
    let request_id = decoder.u32("stream problem request ID")?;
    let code = StreamConfigurationErrorCode::try_from(decoder.u8("stream problem code")?)?;
    let message_length = decoder.u16("stream problem message length")? as usize;
    let message = decoder.utf8(message_length, "stream configuration message")?;
    decoder.finish(field)?;
    Ok((request_id, code, message))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamConfigurationReject {
    pub request_id: u32,
    pub reason: StreamConfigurationErrorCode,
    pub message: String,
}

impl WireCodec for StreamConfigurationReject {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        encode_stream_problem(self.request_id, self.reason, &self.message)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let (request_id, reason, message) =
            decode_stream_problem(input, "stream configuration reject")?;
        Ok(Self {
            request_id,
            reason,
            message,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamConfigurationError {
    pub request_id: u32,
    pub error_code: StreamConfigurationErrorCode,
    pub message: String,
}

impl WireCodec for StreamConfigurationError {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        encode_stream_problem(self.request_id, self.error_code, &self.message)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let (request_id, error_code, message) =
            decode_stream_problem(input, "stream configuration error")?;
        Ok(Self {
            request_id,
            error_code,
            message,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClipboardSyncDirection {
    HostToClient = 0,
    ClientToHost = 1,
    Bidirectional = 2,
}

impl TryFrom<u8> for ClipboardSyncDirection {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::HostToClient),
            1 => Ok(Self::ClientToHost),
            2 => Ok(Self::Bidirectional),
            unknown => Err(CodecError::UnknownClipboardDirection(unknown)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClipboardSyncOrigin {
    LocalPasteboard = 0,
    RemotePasteboard = 1,
    SyncedFromPeer = 2,
}

impl TryFrom<u8> for ClipboardSyncOrigin {
    type Error = CodecError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::LocalPasteboard),
            1 => Ok(Self::RemotePasteboard),
            2 => Ok(Self::SyncedFromPeer),
            unknown => Err(CodecError::UnknownClipboardOrigin(unknown)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardSyncRequest {
    pub request_id: u32,
    pub direction: ClipboardSyncDirection,
    pub origin: ClipboardSyncOrigin,
}

impl ClipboardSyncRequest {
    pub const SIZE: usize = 6;
}

impl WireCodec for ClipboardSyncRequest {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = Vec::with_capacity(Self::SIZE);
        push_u32(&mut output, self.request_id);
        output.push(self.direction as u8);
        output.push(self.origin as u8);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let request_id = decoder.u32("clipboard request ID")?;
        let direction = ClipboardSyncDirection::try_from(decoder.u8("clipboard direction")?)?;
        let origin = ClipboardSyncOrigin::try_from(decoder.u8("clipboard origin")?)?;
        decoder.finish("clipboard sync request")?;
        Ok(Self {
            request_id,
            direction,
            origin,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardSyncUpdate {
    pub request_id: u32,
    pub direction: ClipboardSyncDirection,
    pub origin: ClipboardSyncOrigin,
    pub text: String,
}

impl WireCodec for ClipboardSyncUpdate {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let text = checked_bytes(&self.text, "clipboard text", MAX_CLIPBOARD_TEXT_BYTES)?;
        let mut output = Vec::with_capacity(8 + text.len());
        push_u32(&mut output, self.request_id);
        output.push(self.direction as u8);
        output.push(self.origin as u8);
        push_u16(&mut output, usize_to_u16(text.len(), "clipboard text")?);
        output.extend_from_slice(text);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let request_id = decoder.u32("clipboard update ID")?;
        let direction = ClipboardSyncDirection::try_from(decoder.u8("clipboard direction")?)?;
        let origin = ClipboardSyncOrigin::try_from(decoder.u8("clipboard origin")?)?;
        let text_length = decoder.u16("clipboard text length")? as usize;
        if text_length > MAX_CLIPBOARD_TEXT_BYTES {
            return Err(CodecError::LengthLimit {
                field: "clipboard text",
                actual: text_length,
                max: MAX_CLIPBOARD_TEXT_BYTES,
            });
        }
        let text = decoder.utf8(text_length, "clipboard text")?;
        decoder.finish("clipboard sync update")?;
        Ok(Self {
            request_id,
            direction,
            origin,
            text,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardSyncError {
    pub request_id: u32,
    pub direction: ClipboardSyncDirection,
    pub origin: ClipboardSyncOrigin,
    pub error_code: u8,
    pub message: String,
}

impl WireCodec for ClipboardSyncError {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let message = checked_bytes(&self.message, "clipboard error message", u16::MAX as usize)?;
        let mut output = Vec::with_capacity(9 + message.len());
        push_u32(&mut output, self.request_id);
        output.push(self.direction as u8);
        output.push(self.origin as u8);
        output.push(self.error_code);
        push_u16(
            &mut output,
            usize_to_u16(message.len(), "clipboard error message")?,
        );
        output.extend_from_slice(message);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let request_id = decoder.u32("clipboard error ID")?;
        let direction = ClipboardSyncDirection::try_from(decoder.u8("clipboard direction")?)?;
        let origin = ClipboardSyncOrigin::try_from(decoder.u8("clipboard origin")?)?;
        let error_code = decoder.u8("clipboard error code")?;
        let message_length = decoder.u16("clipboard error message length")? as usize;
        let message = decoder.utf8(message_length, "clipboard error message")?;
        decoder.finish("clipboard sync error")?;
        Ok(Self {
            request_id,
            direction,
            origin,
            error_code,
            message,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlMessage {
    RequestKeyFrame,
    StartStream,
    StopStream,
    Disconnect,
    Ping,
    Pong,
    BitrateAdjust(BitrateAdjust),
    StreamConfigRequest(StreamConfigurationRequest),
    StreamConfigResponse(StreamConfigurationResponse),
    StreamConfigReject(StreamConfigurationReject),
    StreamConfigError(StreamConfigurationError),
    ClipboardSyncRequest(ClipboardSyncRequest),
    ClipboardSyncUpdate(ClipboardSyncUpdate),
    ClipboardSyncError(ClipboardSyncError),
    InputAck(InputAckMessage),
}

impl ControlMessage {
    pub const fn message_type(&self) -> ControlMessageType {
        match self {
            Self::RequestKeyFrame => ControlMessageType::RequestKeyFrame,
            Self::StartStream => ControlMessageType::StartStream,
            Self::StopStream => ControlMessageType::StopStream,
            Self::Disconnect => ControlMessageType::Disconnect,
            Self::Ping => ControlMessageType::Ping,
            Self::Pong => ControlMessageType::Pong,
            Self::BitrateAdjust(_) => ControlMessageType::BitrateAdjust,
            Self::StreamConfigRequest(_) => ControlMessageType::StreamConfigRequest,
            Self::StreamConfigResponse(_) => ControlMessageType::StreamConfigResponse,
            Self::StreamConfigReject(_) => ControlMessageType::StreamConfigReject,
            Self::StreamConfigError(_) => ControlMessageType::StreamConfigError,
            Self::ClipboardSyncRequest(_) => ControlMessageType::ClipboardSyncRequest,
            Self::ClipboardSyncUpdate(_) => ControlMessageType::ClipboardSyncUpdate,
            Self::ClipboardSyncError(_) => ControlMessageType::ClipboardSyncError,
            Self::InputAck(_) => ControlMessageType::InputAck,
        }
    }
}

impl WireCodec for ControlMessage {
    fn encode(&self) -> Result<Vec<u8>, CodecError> {
        let mut output = vec![self.message_type() as u8];
        let body = match self {
            Self::RequestKeyFrame
            | Self::StartStream
            | Self::StopStream
            | Self::Disconnect
            | Self::Ping
            | Self::Pong => return Ok(output),
            Self::BitrateAdjust(value) => value.encode()?,
            Self::StreamConfigRequest(value) => value.encode()?,
            Self::StreamConfigResponse(value) => value.encode()?,
            Self::StreamConfigReject(value) => value.encode()?,
            Self::StreamConfigError(value) => value.encode()?,
            Self::ClipboardSyncRequest(value) => value.encode()?,
            Self::ClipboardSyncUpdate(value) => value.encode()?,
            Self::ClipboardSyncError(value) => value.encode()?,
            Self::InputAck(value) => value.encode()?,
        };
        output.extend_from_slice(&body);
        Ok(output)
    }

    fn decode(input: &[u8]) -> Result<Self, CodecError> {
        let mut decoder = Decoder::new(input);
        let message_type = ControlMessageType::try_from(decoder.u8("control message type")?)?;
        let body = decoder.take_remaining();
        match message_type {
            ControlMessageType::RequestKeyFrame => {
                Decoder::new(body).finish("control message")?;
                Ok(Self::RequestKeyFrame)
            }
            ControlMessageType::StartStream => {
                Decoder::new(body).finish("control message")?;
                Ok(Self::StartStream)
            }
            ControlMessageType::StopStream => {
                Decoder::new(body).finish("control message")?;
                Ok(Self::StopStream)
            }
            ControlMessageType::Disconnect => {
                Decoder::new(body).finish("control message")?;
                Ok(Self::Disconnect)
            }
            ControlMessageType::Ping => {
                Decoder::new(body).finish("control message")?;
                Ok(Self::Ping)
            }
            ControlMessageType::Pong => {
                Decoder::new(body).finish("control message")?;
                Ok(Self::Pong)
            }
            ControlMessageType::BitrateAdjust => {
                Ok(Self::BitrateAdjust(BitrateAdjust::decode(body)?))
            }
            ControlMessageType::StreamConfigRequest => Ok(Self::StreamConfigRequest(
                StreamConfigurationRequest::decode(body)?,
            )),
            ControlMessageType::StreamConfigResponse => Ok(Self::StreamConfigResponse(
                StreamConfigurationResponse::decode(body)?,
            )),
            ControlMessageType::StreamConfigReject => Ok(Self::StreamConfigReject(
                StreamConfigurationReject::decode(body)?,
            )),
            ControlMessageType::StreamConfigError => Ok(Self::StreamConfigError(
                StreamConfigurationError::decode(body)?,
            )),
            ControlMessageType::ClipboardSyncRequest => Ok(Self::ClipboardSyncRequest(
                ClipboardSyncRequest::decode(body)?,
            )),
            ControlMessageType::ClipboardSyncUpdate => Ok(Self::ClipboardSyncUpdate(
                ClipboardSyncUpdate::decode(body)?,
            )),
            ControlMessageType::ClipboardSyncError => {
                Ok(Self::ClipboardSyncError(ClipboardSyncError::decode(body)?))
            }
            ControlMessageType::InputAck => Ok(Self::InputAck(InputAckMessage::decode(body)?)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_ack_message_roundtrip() {
        let original = InputAckMessage {
            sequence: 42,
            success: true,
            error_code: 0,
        };
        let encoded = original.encode().unwrap();
        assert_eq!(encoded.len(), InputAckMessage::SIZE);
        let decoded = InputAckMessage::decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn input_ack_message_with_error() {
        let original = InputAckMessage {
            sequence: 100,
            success: false,
            error_code: 5,
        };
        let encoded = original.encode().unwrap();
        assert_eq!(encoded.len(), InputAckMessage::SIZE);
        let decoded = InputAckMessage::decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn bodyless_control_message_rejects_trailing_bytes() {
        // A bodyless control value followed by an extra byte must not decode;
        // previously the suffix was silently discarded.
        let malformed = [ControlMessageType::StartStream as u8, 0x03];
        assert!(matches!(
            ControlMessage::decode(&malformed),
            Err(CodecError::TrailingBytes { .. })
        ));
        let ping_suffix = [ControlMessageType::Ping as u8, 0xff, 0x00];
        assert!(matches!(
            ControlMessage::decode(&ping_suffix),
            Err(CodecError::TrailingBytes { .. })
        ));
        // Exact-length bodyless messages still decode.
        assert_eq!(
            ControlMessage::decode(&[ControlMessageType::StartStream as u8]).unwrap(),
            ControlMessage::StartStream
        );
    }

    #[test]
    fn control_message_input_ack_roundtrip() {
        let ack = InputAckMessage {
            sequence: 123,
            success: true,
            error_code: 0,
        };
        let original = ControlMessage::InputAck(ack);
        let encoded = original.encode().unwrap();
        assert!(!encoded.is_empty());
        assert_eq!(encoded[0], ControlMessageType::InputAck as u8);
        let decoded = ControlMessage::decode(&encoded).unwrap();
        assert_eq!(decoded, original);
    }
}
