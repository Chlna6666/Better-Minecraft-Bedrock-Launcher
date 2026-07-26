use crate::{NethernetError, Result};
use bytes::{Buf as _, Bytes, BytesMut};
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, Notify, OnceCell, mpsc};
use tokio_util::sync::CancellationToken;
use webrtc::data_channel::RTCDataChannel;
use webrtc::peer_connection::RTCPeerConnection;

const SEGMENT_PAYLOAD_SIZE: usize = 256 * 1024 - 1;
const MAX_SEGMENTS: usize = u8::MAX as usize + 1;
const MAX_PACKET_SIZE: usize = SEGMENT_PAYLOAD_SIZE * MAX_SEGMENTS;
const PACKET_QUEUE_CAPACITY: usize = 64;

pub struct NethernetSession {
    peer_connection: Arc<RTCPeerConnection>,
    reliable_channel: OnceCell<Arc<RTCDataChannel>>,
    unreliable_channel: OnceCell<Arc<RTCDataChannel>>,
    reliable_send_lock: Mutex<()>,
    packet_sender: mpsc::Sender<Bytes>,
    packet_receiver: Mutex<mpsc::Receiver<Bytes>>,
    closed: Arc<AtomicBool>,
    closed_notify: Arc<Notify>,
    background_cancel: CancellationToken,
}

impl NethernetSession {
    pub(crate) fn new(peer_connection: Arc<RTCPeerConnection>) -> Self {
        let (packet_sender, packet_receiver) = mpsc::channel(PACKET_QUEUE_CAPACITY);
        Self {
            peer_connection,
            reliable_channel: OnceCell::new(),
            unreliable_channel: OnceCell::new(),
            reliable_send_lock: Mutex::new(()),
            packet_sender,
            packet_receiver: Mutex::new(packet_receiver),
            closed: Arc::new(AtomicBool::new(false)),
            closed_notify: Arc::new(Notify::new()),
            background_cancel: CancellationToken::new(),
        }
    }

    pub(crate) fn attach_reliable(&self, channel: Arc<RTCDataChannel>) {
        let assembler = Arc::new(StdMutex::new(MessageAssembler::default()));
        let packet_sender = self.packet_sender.clone();
        channel.on_message(Box::new(move |message| {
            let assembler = Arc::clone(&assembler);
            let packet_sender = packet_sender.clone();
            Box::pin(async move {
                let result = assembler
                    .lock()
                    .map_err(|_| NethernetError::Protocol("NetherNet 重组状态锁已损坏".to_string()))
                    .and_then(|mut assembler| assembler.push(message.data));
                match result {
                    Ok(Some(packet)) => enqueue_packet(&packet_sender, packet).await,
                    Ok(None) => {}
                    Err(error) => tracing::warn!("丢弃无效 NetherNet 可靠消息：{error}"),
                }
            })
        }));
        self.install_close_handler(&channel);
        if self.reliable_channel.set(channel).is_err() {
            tracing::warn!("忽略重复的 NetherNet 可靠数据通道");
        }
    }

    pub(crate) fn attach_unreliable(&self, channel: Arc<RTCDataChannel>) {
        let packet_sender = self.packet_sender.clone();
        channel.on_message(Box::new(move |message| {
            let packet_sender = packet_sender.clone();
            Box::pin(async move {
                let mut data = message.data;
                if data.len() < 2 || data[0] != 0 {
                    tracing::warn!("丢弃无效 NetherNet 不可靠消息");
                    return;
                }
                data.advance(1);
                enqueue_packet(&packet_sender, data).await;
            })
        }));
        if self.unreliable_channel.set(channel).is_err() {
            tracing::warn!("忽略重复的 NetherNet 不可靠数据通道");
        }
    }

    fn install_close_handler(&self, channel: &RTCDataChannel) {
        let closed = Arc::clone(&self.closed);
        let closed_notify = Arc::clone(&self.closed_notify);
        channel.on_close(Box::new(move || {
            let closed = Arc::clone(&closed);
            let closed_notify = Arc::clone(&closed_notify);
            Box::pin(async move {
                closed.store(true, Ordering::Release);
                closed_notify.notify_waiters();
            })
        }));
    }

    pub(crate) fn channels_ready(&self) -> bool {
        self.reliable_channel.get().is_some() && self.unreliable_channel.get().is_some()
    }

    pub(crate) fn cancellation_token(&self) -> CancellationToken {
        self.background_cancel.clone()
    }

    /// Sends one complete packet over the ordered reliable data channel.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is closed, the packet exceeds the
    /// protocol limit, or the WebRTC channel rejects a segment.
    pub async fn send(&self, data: Bytes) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(NethernetError::Closed);
        }
        if data.is_empty() {
            return Err(NethernetError::Protocol(
                "NetherNet 不允许发送空数据包".to_string(),
            ));
        }
        let segment_count = data.len().div_ceil(SEGMENT_PAYLOAD_SIZE);
        if segment_count > MAX_SEGMENTS {
            return Err(NethernetError::Protocol(format!(
                "NetherNet 数据包超过 {MAX_PACKET_SIZE} 字节上限"
            )));
        }
        let channel = self
            .reliable_channel
            .get()
            .cloned()
            .ok_or(NethernetError::Closed)?;
        let _send_guard = self.reliable_send_lock.lock().await;
        let mut remaining = data;
        for remaining_segments in (0..segment_count).rev() {
            let length = remaining.len().min(SEGMENT_PAYLOAD_SIZE);
            let chunk = remaining.split_to(length);
            let mut frame = BytesMut::with_capacity(chunk.len() + 1);
            frame.extend_from_slice(&[u8::try_from(remaining_segments)
                .map_err(|_| NethernetError::Protocol("NetherNet 分片数量溢出".to_string()))?]);
            frame.extend_from_slice(&chunk);
            channel.send(&frame.freeze()).await?;
        }
        Ok(())
    }

    /// Receives the next complete packet, or `None` after the session closes.
    ///
    /// # Errors
    ///
    /// Returns an error when the receive queue cannot be accessed.
    pub async fn recv(&self) -> Result<Option<Bytes>> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(None);
        }
        let mut packet_receiver = self.packet_receiver.lock().await;
        tokio::select! {
            packet = packet_receiver.recv() => Ok(packet),
            () = self.closed_notify.notified() => Ok(None),
        }
    }

    /// Closes both data channels and the underlying peer connection.
    ///
    /// # Errors
    ///
    /// Returns an error when WebRTC peer shutdown fails.
    pub async fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.closed_notify.notify_waiters();
        self.background_cancel.cancel();
        if let Some(channel) = self.reliable_channel.get()
            && let Err(error) = channel.close().await
        {
            tracing::debug!("关闭 NetherNet 可靠通道失败：{error}");
        }
        if let Some(channel) = self.unreliable_channel.get()
            && let Err(error) = channel.close().await
        {
            tracing::debug!("关闭 NetherNet 不可靠通道失败：{error}");
        }
        self.peer_connection.close().await?;
        Ok(())
    }
}

async fn enqueue_packet(sender: &mpsc::Sender<Bytes>, packet: Bytes) {
    match sender.try_send(packet) {
        Ok(()) => {}
        Err(mpsc::error::TrySendError::Full(packet)) => {
            if sender.send(packet).await.is_err() {
                tracing::trace!("NetherNet 接收队列已关闭");
            }
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            tracing::trace!("NetherNet 接收队列已关闭");
        }
    }
}

#[derive(Default)]
struct MessageAssembler {
    next_remaining: Option<u8>,
    data: BytesMut,
}

impl MessageAssembler {
    fn push(&mut self, mut frame: Bytes) -> Result<Option<Bytes>> {
        if frame.len() < 2 {
            self.reset();
            return Err(NethernetError::Protocol(
                "NetherNet 数据通道消息过短".to_string(),
            ));
        }
        let remaining = frame[0];
        frame.advance(1);
        if remaining == 0 && self.next_remaining.is_none() && self.data.is_empty() {
            return Ok(Some(frame));
        }
        match self.next_remaining {
            None if remaining > 0 => {
                self.next_remaining = remaining.checked_sub(1);
            }
            None => {}
            Some(expected) if expected == remaining => {
                self.next_remaining = remaining.checked_sub(1);
            }
            Some(expected) => {
                self.reset();
                return Err(NethernetError::Protocol(format!(
                    "NetherNet 分片顺序无效：期望 {expected}，收到 {remaining}"
                )));
            }
        }
        let new_length = self
            .data
            .len()
            .checked_add(frame.len())
            .ok_or_else(|| NethernetError::Protocol("NetherNet 数据包长度溢出".to_string()))?;
        if new_length > MAX_PACKET_SIZE {
            self.reset();
            return Err(NethernetError::Protocol(
                "NetherNet 重组数据包过大".to_string(),
            ));
        }
        self.data.extend_from_slice(&frame);
        if remaining == 0 {
            self.next_remaining = None;
            return Ok(Some(self.data.split().freeze()));
        }
        Ok(None)
    }

    fn reset(&mut self) {
        self.next_remaining = None;
        self.data.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_PACKET_SIZE, MessageAssembler, NethernetSession};
    use bytes::Bytes;

    #[test]
    fn reassembles_ordered_segments() {
        let mut assembler = MessageAssembler::default();
        assert_eq!(
            assembler.push(Bytes::from_static(b"\x01hello ")).unwrap(),
            None
        );
        assert_eq!(
            assembler.push(Bytes::from_static(b"\x00world")).unwrap(),
            Some(Bytes::from_static(b"hello world"))
        );
    }

    #[test]
    fn rejects_out_of_order_segments() {
        let mut assembler = MessageAssembler::default();
        assembler.push(Bytes::from_static(b"\x02a")).unwrap();
        assert!(assembler.push(Bytes::from_static(b"\x00b")).is_err());
    }

    #[test]
    fn packet_limit_matches_protocol_segment_field() {
        assert_eq!(MAX_PACKET_SIZE, (256 * 1024 - 1) * 256);
    }

    #[test]
    fn single_segment_reuses_the_received_allocation() {
        let mut assembler = MessageAssembler::default();
        let frame = Bytes::from_static(b"\x00payload");
        let expected_pointer = frame[1..].as_ptr();
        let packet = assembler
            .push(frame)
            .expect("parse segment")
            .expect("complete packet");
        assert_eq!(packet.as_ptr(), expected_pointer);
    }

    #[test]
    fn session_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NethernetSession>();
    }
}
