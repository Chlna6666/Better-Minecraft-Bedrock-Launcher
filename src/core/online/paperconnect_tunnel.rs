use bytes::Bytes;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const PACKET_FRAME: u8 = 1;
const CHUNK_FRAME: u8 = 2;
/// 本端编码分片大小：保持在本端 RakNet MTU（1200）之下，避免单帧再被拆分。
const CHUNK_SIZE: usize = 900;
/// 对端分片大小上限：兼容协议只约定帧头格式，不约定分片大小。
/// GravityCone/PaperConnect 房主按 1349 字节分片（MTU 1400），
/// 这里只做内存防护，不能用本端 CHUNK_SIZE 校验对端数据。
const MAX_REMOTE_CHUNK_SIZE: usize = 4096;
const MAX_CHUNKS: usize = 1024;
/// 本端出站数据包大小上限（受本端分片大小限制）。
const MAX_PACKET_SIZE: usize = CHUNK_SIZE * MAX_CHUNKS;
/// 对端入站重组数据包大小上限。
const MAX_REMOTE_PACKET_SIZE: usize = MAX_REMOTE_CHUNK_SIZE * MAX_CHUNKS;
const MAX_INCOMPLETE_MESSAGES: usize = 128;
const ASSEMBLY_TTL: Duration = Duration::from_secs(30);

static NEXT_MESSAGE_ID: AtomicU32 = AtomicU32::new(1);

pub fn encode_packet(packet: &[u8]) -> Result<Vec<Box<[u8]>>, String> {
    if packet.len() > MAX_PACKET_SIZE {
        return Err(format!(
            "PaperConnect 隧道数据包过大：{} > {MAX_PACKET_SIZE}",
            packet.len()
        ));
    }
    if packet.len() <= CHUNK_SIZE {
        let length = u32::try_from(packet.len())
            .map_err(|_| "PaperConnect 隧道数据包长度溢出".to_string())?;
        let mut frame = Vec::with_capacity(5 + packet.len());
        frame.push(PACKET_FRAME);
        frame.extend_from_slice(&length.to_be_bytes());
        frame.extend_from_slice(packet);
        return Ok(vec![frame.into_boxed_slice()]);
    }

    let chunk_count = packet.len().div_ceil(CHUNK_SIZE);
    let chunk_count_u16 =
        u16::try_from(chunk_count).map_err(|_| "PaperConnect 隧道分片数量溢出".to_string())?;
    let message_id = NEXT_MESSAGE_ID.fetch_add(1, Ordering::Relaxed);
    let mut frames = Vec::with_capacity(chunk_count);
    for (chunk_index, chunk) in packet.chunks(CHUNK_SIZE).enumerate() {
        let chunk_index =
            u16::try_from(chunk_index).map_err(|_| "PaperConnect 隧道分片序号溢出".to_string())?;
        let mut frame = Vec::with_capacity(9 + chunk.len());
        frame.push(CHUNK_FRAME);
        frame.extend_from_slice(&message_id.to_be_bytes());
        frame.extend_from_slice(&chunk_count_u16.to_be_bytes());
        frame.extend_from_slice(&chunk_index.to_be_bytes());
        frame.extend_from_slice(chunk);
        frames.push(frame.into_boxed_slice());
    }
    Ok(frames)
}

struct IncompleteMessage {
    created_at: Instant,
    received_bytes: usize,
    chunks: Vec<Option<Bytes>>,
}

#[derive(Default)]
pub struct TunnelDecoder {
    messages: HashMap<u32, IncompleteMessage>,
    insertion_order: VecDeque<u32>,
}

impl TunnelDecoder {
    pub fn push(&mut self, frame: Bytes) -> Result<Option<Bytes>, String> {
        self.remove_expired();
        let Some(frame_type) = frame.first().copied() else {
            return Err("PaperConnect 隧道帧为空".to_string());
        };
        match frame_type {
            PACKET_FRAME => decode_packet_frame(frame),
            CHUNK_FRAME => self.decode_chunk_frame(frame),
            _ => Err(format!("未知 PaperConnect 隧道帧类型：{frame_type}")),
        }
    }

    fn decode_chunk_frame(&mut self, frame: Bytes) -> Result<Option<Bytes>, String> {
        if frame.len() < 9 {
            return Err("PaperConnect 隧道分片帧过短".to_string());
        }
        let message_id = u32::from_be_bytes(
            frame[1..5]
                .try_into()
                .map_err(|_| "PaperConnect 隧道消息编号无效".to_string())?,
        );
        let chunk_count = usize::from(u16::from_be_bytes(
            frame[5..7]
                .try_into()
                .map_err(|_| "PaperConnect 隧道分片数量无效".to_string())?,
        ));
        let chunk_index = usize::from(u16::from_be_bytes(
            frame[7..9]
                .try_into()
                .map_err(|_| "PaperConnect 隧道分片序号无效".to_string())?,
        ));
        let chunk = frame.slice(9..);
        if chunk_count == 0 || chunk_count > MAX_CHUNKS || chunk_index >= chunk_count {
            return Err(format!(
                "PaperConnect 隧道分片位置无效：{chunk_index}/{chunk_count}"
            ));
        }
        if chunk.is_empty() || chunk.len() > MAX_REMOTE_CHUNK_SIZE {
            return Err(format!("PaperConnect 隧道分片长度无效：{}", chunk.len()));
        }

        if !self.messages.contains_key(&message_id) {
            self.make_capacity();
            self.insertion_order.push_back(message_id);
            self.messages.insert(
                message_id,
                IncompleteMessage {
                    created_at: Instant::now(),
                    received_bytes: 0,
                    chunks: vec![None; chunk_count],
                },
            );
        }

        let message = self
            .messages
            .get_mut(&message_id)
            .ok_or_else(|| "PaperConnect 隧道分片状态丢失".to_string())?;
        if message.chunks.len() != chunk_count {
            self.remove_message(message_id);
            return Err("PaperConnect 隧道分片数量前后不一致".to_string());
        }
        if message.chunks[chunk_index].is_none() {
            message.received_bytes = message.received_bytes.saturating_add(chunk.len());
            if message.received_bytes > MAX_REMOTE_PACKET_SIZE {
                self.remove_message(message_id);
                return Err("PaperConnect 隧道重组数据包过大".to_string());
            }
            message.chunks[chunk_index] = Some(chunk);
        }
        if message.chunks.iter().any(Option::is_none) {
            return Ok(None);
        }

        let message = self
            .messages
            .remove(&message_id)
            .ok_or_else(|| "PaperConnect 隧道重组状态丢失".to_string())?;
        self.insertion_order.retain(|id| *id != message_id);
        let mut packet = Vec::with_capacity(message.received_bytes);
        for chunk in message.chunks.into_iter().flatten() {
            packet.extend_from_slice(&chunk);
        }
        Ok(Some(Bytes::from(packet)))
    }

    fn make_capacity(&mut self) {
        while self.messages.len() >= MAX_INCOMPLETE_MESSAGES {
            let Some(message_id) = self.insertion_order.pop_front() else {
                break;
            };
            self.messages.remove(&message_id);
        }
    }

    fn remove_expired(&mut self) {
        let now = Instant::now();
        self.messages
            .retain(|_, message| now.duration_since(message.created_at) <= ASSEMBLY_TTL);
        self.insertion_order
            .retain(|message_id| self.messages.contains_key(message_id));
    }

    fn remove_message(&mut self, message_id: u32) {
        self.messages.remove(&message_id);
        self.insertion_order.retain(|id| *id != message_id);
    }
}

fn decode_packet_frame(mut frame: Bytes) -> Result<Option<Bytes>, String> {
    if frame.len() < 5 {
        return Err("PaperConnect 隧道数据帧过短".to_string());
    }
    let packet_length = usize::try_from(u32::from_be_bytes(
        frame[1..5]
            .try_into()
            .map_err(|_| "PaperConnect 隧道数据长度无效".to_string())?,
    ))
    .map_err(|_| "PaperConnect 隧道数据长度溢出".to_string())?;
    if packet_length > MAX_REMOTE_PACKET_SIZE || packet_length != frame.len() - 5 {
        return Err(format!(
            "PaperConnect 隧道数据长度不匹配：声明 {packet_length}，实际 {}",
            frame.len() - 5
        ));
    }
    Ok(Some(frame.split_off(5)))
}

#[cfg(test)]
mod tests {
    use super::{CHUNK_SIZE, TunnelDecoder, encode_packet};
    use bytes::Bytes;

    #[test]
    fn small_packet_round_trips() {
        let mut frames = encode_packet(b"paperconnect-v2").expect("small packet should encode");
        assert_eq!(frames.len(), 1);
        let packet = TunnelDecoder::default()
            .push(Bytes::from(frames.remove(0)))
            .expect("frame should decode")
            .expect("single frame should complete");
        assert_eq!(packet.as_ref(), b"paperconnect-v2");
    }

    #[test]
    fn chunked_packet_round_trips_out_of_order() {
        let packet = vec![0x5a; CHUNK_SIZE * 3 + 17];
        let mut frames = encode_packet(&packet).expect("large packet should encode");
        frames.reverse();
        let mut decoder = TunnelDecoder::default();
        let mut decoded = None;
        for frame in frames {
            if let Some(packet) = decoder
                .push(Bytes::from(frame))
                .expect("chunk should decode")
            {
                decoded = Some(packet);
            }
        }
        assert_eq!(
            decoded.expect("all chunks should complete").as_ref(),
            packet
        );
    }

    /// GravityCone/PaperConnect 按 1349 字节分片（MTU 1400）。
    /// 按其 writeTunnelPacket 的字节布局构造帧，解码端必须能接受。
    #[test]
    fn gravitycone_sized_chunks_round_trip() {
        const GRAVITYCONE_CHUNK_SIZE: usize = 1349;
        let packet: Vec<u8> = (0..GRAVITYCONE_CHUNK_SIZE * 2 + 321)
            .map(|byte| byte as u8)
            .collect();
        let chunks: Vec<&[u8]> = packet.chunks(GRAVITYCONE_CHUNK_SIZE).collect();
        let mut decoder = TunnelDecoder::default();
        let mut decoded = None;
        for (index, chunk) in chunks.iter().enumerate() {
            let mut frame = Vec::with_capacity(9 + chunk.len());
            frame.push(2); // tunnelChunk
            frame.extend_from_slice(&7_u32.to_be_bytes()); // messageID
            frame.extend_from_slice(&(chunks.len() as u16).to_be_bytes());
            frame.extend_from_slice(&(index as u16).to_be_bytes());
            frame.extend_from_slice(chunk);
            if let Some(packet) = decoder
                .push(Bytes::from(frame))
                .expect("GravityCone-sized chunk should decode")
            {
                decoded = Some(packet);
            }
        }
        assert_eq!(decoded.expect("chunks should reassemble").as_ref(), packet);
    }

    #[test]
    fn gravitycone_sized_single_packet_frame_decodes() {
        let packet = vec![0x3c; 1349];
        let mut frame = Vec::with_capacity(5 + packet.len());
        frame.push(1); // tunnelPacket
        frame.extend_from_slice(&(packet.len() as u32).to_be_bytes());
        frame.extend_from_slice(&packet);
        let decoded = TunnelDecoder::default()
            .push(Bytes::from(frame))
            .expect("GravityCone-sized packet frame should decode")
            .expect("single frame should complete");
        assert_eq!(decoded.as_ref(), packet);
    }

    #[test]
    fn malformed_length_is_rejected() {
        let frame = [1, 0, 0, 0, 8, 1, 2];
        assert!(
            TunnelDecoder::default()
                .push(Bytes::copy_from_slice(&frame))
                .is_err()
        );
    }

    #[test]
    fn small_packet_decode_reuses_raknet_frame_allocation() {
        let mut frames = encode_packet(b"payload").expect("encode packet");
        let frame = Bytes::from(frames.remove(0));
        let expected_pointer = frame[5..].as_ptr();
        let packet = TunnelDecoder::default()
            .push(frame)
            .expect("decode frame")
            .expect("complete packet");
        assert_eq!(packet.as_ptr(), expected_pointer);
    }
}
