use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use std::io::Error;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageId {
    Choke         = 0,
    Unchoke       = 1,
    Interested    = 2,
    NotInterested = 3,
    Have          = 4,
    Bitfield      = 5,
    Request       = 6,
    Piece         = 7,
    Cancel        = 8,
}

pub enum Message {
    KeepAlive,
    Choke,
    Unchoke,
    Interested,
    NotInterested,
    Have { index: u32 },
    Bitfield { bitmap: Vec<u8> },
    Request { index: u32, begin: u32, length: u32 },
    Piece { index: u32, begin: u32, bytes: Vec<u8> },
    Cancel { index: u32, begin: u32, length: u32 },
}

pub enum MessageError {
    UnknownMessage,
    ReceiveError(Error),
    TransmitError(Error),
    ByteConversionError,
    UnexpectedNumBytes{expected: usize, received: usize},
}

impl TryFrom<u8> for MessageId {
    type Error = MessageError;

    fn try_from(value: u8) -> Result<Self, MessageError> {
        match value {
            0 => Ok(MessageId::Choke),
            1 => Ok(MessageId::Unchoke),
            2 => Ok(MessageId::Interested),
            3 => Ok(MessageId::NotInterested),
            4 => Ok(MessageId::Have),
            5 => Ok(MessageId::Bitfield),
            6 => Ok(MessageId::Request),
            7 => Ok(MessageId::Piece),
            8 => Ok(MessageId::Cancel),
            _ => Err(MessageError::UnknownMessage),
        }
    }
}

impl Message {
    pub async fn read_message(stream: &mut TcpStream) -> Result<Self, MessageError> {
        let mut buf: [u8; 4] = [0; 4];
        Message::read_bytes(stream, &mut buf).await?;

        let total_length = usize::from_be_bytes(buf[0..4].try_into().map_err(|_| MessageError::ByteConversionError)?);
        if total_length == 0 {
            return Ok(Message::KeepAlive)
        }

        let mut id_buf: [u8; 1] = [0; 1];
        Message::read_bytes(stream, &mut id_buf).await?;
        let id: MessageId = MessageId::try_from(id_buf[0])?;

        let payload_length = total_length - 1;
        match id {
            MessageId::Bitfield => Message::read_bitfield(stream, payload_length).await,
            MessageId::Piece => Message::read_piece(stream, payload_length).await,
            MessageId::Have => Message::read_have(stream, payload_length).await,
            MessageId::Request => Message::read_12(stream, true, payload_length).await,
            MessageId::Cancel => Message::read_12(stream, false, payload_length).await,
            _ => Message::read_zero(stream, id, payload_length).await,
        }
    }

    async fn read_bitfield(stream: &mut TcpStream, payload_length: usize) -> Result<Self, MessageError> {
        let bitmap = Message::read_variable_message(stream, payload_length).await?;
        Ok(Message::Bitfield { bitmap })
    }

    async fn read_piece(stream: &mut TcpStream, payload_length: usize) -> Result<Self, MessageError> {
        let mut bytes = Message::read_variable_message(stream, payload_length).await?;
        if bytes.len() < 8 {
            return Err(MessageError::UnexpectedNumBytes { expected: 8, received: bytes.len() });
        }
        let index: u32 = u32::from_be_bytes(bytes[0..4].try_into().map_err(|_| MessageError::ByteConversionError)?);
        let begin: u32 = u32::from_be_bytes(bytes[5..8].try_into().map_err(|_| MessageError::ByteConversionError)?);
        bytes.drain(0..8);
        Ok(Message::Piece{index, begin, bytes})
    }

    async fn read_variable_message(stream: &mut TcpStream, payload_length: usize) -> Result<Vec<u8>, MessageError> {
        let mut v = vec![0u8; payload_length];
        Message::read_bytes(stream, v.as_mut_slice()).await?;
        Ok(v)
    }

    async fn read_bytes(stream: &mut TcpStream, buf: &mut[u8]) -> Result<(), MessageError> {
        let num_read = stream.read_exact(buf).await.map_err(|e| MessageError::ReceiveError(e))?;
        if num_read < buf.len() {
            return Err(MessageError::UnexpectedNumBytes { expected: buf.len(), received: num_read });
        }
        Ok(())
    }

    async fn read_have(stream: &mut TcpStream, payload_length: usize) -> Result<Self, MessageError> {
        let mut buf: [u8; 4] = [0; 4];
        Message::read_bytes(stream, &mut buf).await?;
        Message::consume(stream, payload_length, 4).await?;
        let index = u32::from_be_bytes(buf);
        Ok(Message::Have {index})
    }

    async fn read_12(stream: &mut TcpStream, request: bool, payload_length: usize) -> Result<Self, MessageError> {
        let mut buf: [u8; 12] = [0; 12];
        Message::read_bytes(stream, &mut buf).await?;
        Message::consume(stream, payload_length, 12).await?;
        let index = u32::from_be_bytes(buf[0..4].try_into().map_err(|_| MessageError::ByteConversionError)?);
        let begin = u32::from_be_bytes(buf[5..8].try_into().map_err(|_| MessageError::ByteConversionError)?);
        let length = u32::from_be_bytes(buf[8..12].try_into().map_err(|_| MessageError::ByteConversionError)?);
        if request {
            Ok(Message::Request { index, begin, length })
        }
        else {
            Ok(Message::Cancel { index, begin, length })
        }
    }

    async fn read_zero(stream: &mut TcpStream, id: MessageId, payload_length: usize) -> Result<Self, MessageError> {
        Message::consume(stream, payload_length, 0).await?;
        let msg = match id {
            MessageId::Choke => Message::Choke,
            MessageId::Unchoke => Message::Unchoke,
            MessageId::Interested => Message::Interested,
            MessageId::NotInterested => Message::NotInterested,
            _ => return Err(MessageError::UnknownMessage),
        };
        Ok(msg)
    }

    async fn consume(stream: &mut TcpStream, payload_length: usize, expected: usize) -> Result<(), MessageError> {
        if payload_length > expected {
            let extra = payload_length - expected;
            let mut buf = vec![0; extra];
            stream.read_exact(&mut buf).await.map_err(|e| MessageError::ReceiveError(e))?;
            return Err(MessageError::UnexpectedNumBytes { expected, received: payload_length })
        }
        Ok(())
    }

    pub async fn send_keep_alive(stream: &mut TcpStream) -> Result<(), MessageError> {
        let buf: [u8; 4] = [0; 4];
        Message::send_bytes(stream, &buf).await
    }

    pub async fn send_choke(stream: &mut TcpStream) -> Result<(), MessageError> {
        Message::send_header(stream, MessageId::Choke).await
    }

    pub async fn send_unchoke(stream: &mut TcpStream) -> Result<(), MessageError> {
        Message::send_header(stream, MessageId::Unchoke).await
    }

    pub async fn send_interested(stream: &mut TcpStream) -> Result<(), MessageError> {
        Message::send_header(stream, MessageId::Interested).await
    }

    pub async fn send_not_interested(stream: &mut TcpStream) -> Result<(), MessageError> {
        Message::send_header(stream, MessageId::NotInterested).await
    }

    pub async fn send_bitfield(stream: &mut TcpStream, bitmap: &[u8]) -> Result<(), MessageError> {
        let mut buf = vec![0; 4 + 1 + bitmap.len()];
        Message::encode_header(MessageId::Bitfield, 1 + bitmap.len() as u32, &mut buf);
        buf[5..].copy_from_slice(bitmap.iter().as_slice());
        Message::send_bytes(stream, &buf).await
    }

    pub async fn send_piece(stream: &mut TcpStream, index: u32, begin: u32, data: &[u8]) -> Result<(), MessageError> {
        let mut buf = vec![0; 4 + 1 + 8 + data.len()];
        Message::encode_header(MessageId::Piece, 1 + 8 + data.len() as u32, &mut buf);
        buf[5..9].copy_from_slice(index.to_be_bytes().as_slice());
        buf[9..13].copy_from_slice(begin.to_be_bytes().as_slice());
        buf[13..].copy_from_slice(data.iter().as_slice());
        Message::send_bytes(stream, &buf).await
    }

    pub async fn send_have(stream: &mut TcpStream, index: u32) -> Result<(), MessageError> {
        let mut buf: [u8; 5] = [0; 5];
        Message::encode_header(MessageId::Have, 1 + 4, &mut buf);
        buf[5..9].copy_from_slice(index.to_be_bytes().as_slice());
        Message::send_bytes(stream, &buf).await
    }

    pub async fn send_request(stream: &mut TcpStream, index: u32, begin: u32, length: u32) -> Result<(), MessageError> {
        let mut buf: [u8; 17] = [0; 17];
        Message::encode_12(true, index, begin, length, &mut buf);
        Message::send_bytes(stream, &buf).await
    }

    pub async fn send_cancel(stream: &mut TcpStream, index: u32, begin: u32, length: u32) -> Result<(), MessageError> {
        let mut buf: [u8; 17] = [0; 17];
        Message::encode_12(false, index, begin, length, &mut buf);
        Message::send_bytes(stream, &buf).await
    }

    fn encode_12(request: bool, index: u32, begin: u32, length: u32, buf: &mut[u8]) {
        Message::encode_header(if request { MessageId::Request} else { MessageId::Cancel }, 13, buf);
        buf[5..9].copy_from_slice(index.to_be_bytes().as_slice());
        buf[9..13].copy_from_slice(begin.to_be_bytes().as_slice());
        buf[13..17].copy_from_slice(length.to_be_bytes().as_slice());
    }

    fn encode_header(id: MessageId, length: u32, buf: &mut[u8]) {
        buf[0..4].copy_from_slice(length.to_be_bytes().as_slice());
        buf[5] = id as u8;
    }

    async fn send_header(stream: &mut TcpStream, id: MessageId) -> Result<(), MessageError> {
        let mut buf: [u8; 5] = [0; 5];
        Message::encode_header(id, 1, &mut buf);
        Message::send_bytes(stream, &buf).await
    }

    async fn send_bytes(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), MessageError> {
        stream.write_all(bytes).await.map_err(|e| MessageError::TransmitError(e))
    }
}