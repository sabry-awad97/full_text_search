use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use thiserror::Error;
use tokio::{
    sync::{
        broadcast::{self, Receiver, Sender},
        RwLock,
    },
    time::timeout,
};
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, error, info};

/// Maximum number of messages that can be stored in the broadcast channel
const DEFAULT_CHANNEL_SIZE: usize = 1024;
/// Default timeout for WebSocket operations in seconds
const DEFAULT_WS_TIMEOUT_SECS: u64 = 30;

#[derive(Debug, Error)]
pub enum NotificationError {
    #[error("Failed to send notification: {0}")]
    SendError(String),
    #[error("WebSocket error: {0}")]
    WebSocketError(String),
    #[error("Serialization error: {0}")]
    SerializationError(String),
    #[error("Operation timed out")]
    Timeout,
}

/// Represents different types of events that can be broadcasted
#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum Event<T> {
    Created(T),
    Updated(T),
    Deleted(i32),
    /// Custom event type for application-specific events
    Custom {
        event_type: String,
        payload: T,
    },
}

/// Statistics for monitoring the notification system
#[derive(Debug, Default)]
pub struct NotificationStats {
    messages_sent: AtomicU64,
    messages_dropped: AtomicU64,
    active_subscribers: AtomicU64,
    last_broadcast: RwLock<Option<Instant>>,
}

impl NotificationStats {
    fn new() -> Self {
        Self::default()
    }

    fn record_message_sent(&self) {
        self.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    fn record_message_dropped(&self) {
        self.messages_dropped.fetch_add(1, Ordering::Relaxed);
    }

    fn record_subscriber_added(&self) {
        self.active_subscribers.fetch_add(1, Ordering::Relaxed);
    }

    #[allow(dead_code)]
    fn record_subscriber_removed(&self) {
        self.active_subscribers.fetch_sub(1, Ordering::Relaxed);
    }

    async fn update_last_broadcast(&self) {
        *self.last_broadcast.write().await = Some(Instant::now());
    }
}

/// Configuration options for the NotificationHub
#[derive(Debug, Clone)]
pub struct NotificationConfig {
    channel_size: usize,
    ws_timeout: Duration,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            channel_size: DEFAULT_CHANNEL_SIZE,
            ws_timeout: Duration::from_secs(DEFAULT_WS_TIMEOUT_SECS),
        }
    }
}

/// A hub for managing real-time notifications and WebSocket connections
#[derive(Clone)]
pub struct NotificationHub<T> {
    sender: Sender<Event<T>>,
    stats: Arc<NotificationStats>,
    #[allow(dead_code)]
    config: NotificationConfig,
}

impl<T> NotificationHub<T>
where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    /// Creates a new NotificationHub with default configuration
    pub fn new() -> Self {
        Self::with_config(NotificationConfig::default())
    }

    /// Creates a new NotificationHub with custom configuration
    pub fn with_config(config: NotificationConfig) -> Self {
        let (sender, _) = broadcast::channel(config.channel_size);
        Self {
            sender,
            stats: Arc::new(NotificationStats::new()),
            config,
        }
    }

    /// Returns a clone of the sender
    pub fn sender(&self) -> Sender<Event<T>> {
        self.sender.clone()
    }

    /// Creates a new subscriber
    pub fn subscribe(&self) -> Receiver<Event<T>> {
        self.stats.record_subscriber_added();

        self.sender.subscribe()
    }

    /// Broadcasts an event to all subscribers
    pub async fn broadcast(&self, event: Event<T>) -> Result<(), NotificationError> {
        match self.sender.send(event.clone()) {
            Ok(receiver_count) => {
                self.stats.record_message_sent();
                self.stats.update_last_broadcast().await;
                debug!("Event broadcasted to {} receivers", receiver_count);
                Ok(())
            }
            Err(_) => {
                self.stats.record_message_dropped();
                error!("Failed to broadcast event: no active subscribers");
                Err(NotificationError::SendError(format!(
                    "No active subscribers for event: {:?}",
                    event
                )))
            }
        }
    }

    /// Returns the current number of subscribers
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Returns a snapshot of the current statistics
    pub async fn get_stats(&self) -> NotificationStats {
        NotificationStats {
            messages_sent: AtomicU64::new(self.stats.messages_sent.load(Ordering::Relaxed)),
            messages_dropped: AtomicU64::new(self.stats.messages_dropped.load(Ordering::Relaxed)),
            active_subscribers: AtomicU64::new(
                self.stats.active_subscribers.load(Ordering::Relaxed),
            ),
            last_broadcast: RwLock::new(*self.stats.last_broadcast.read().await),
        }
    }

    /// Handles a WebSocket connection with timeout and error handling
    pub async fn handle_socket(
        socket: WebSocket,
        sender: Sender<Event<T>>,
        config: NotificationConfig,
    ) where
        T: Serialize,
    {
        info!("New WebSocket connection established");
        let (sender_ws, mut receiver) = socket.split();
        let mut receiver_stream = BroadcastStream::new(sender.subscribe());

        let send_task = {
            let mut sender_ws = sender_ws;
            tokio::spawn(async move {
                while let Some(Ok(msg)) = receiver_stream.next().await {
                    match serde_json::to_string(&msg) {
                        Ok(json) => {
                            if let Err(e) =
                                timeout(config.ws_timeout, sender_ws.send(Message::Text(json)))
                                    .await
                            {
                                error!("WebSocket send timeout: {}", e);
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Failed to serialize message: {}", e);
                            continue;
                        }
                    }
                }
                debug!("WebSocket send task completed");
            })
        };

        let recv_task = tokio::spawn(async move {
            while let Some(Ok(msg)) = receiver.next().await {
                match msg {
                    Message::Close(_) => {
                        info!("Received close message from client");
                        break;
                    }
                    Message::Ping(_) => {
                        debug!("Received ping message");
                        // Pong is automatically handled by the WebSocket implementation
                    }
                    _ => {
                        // Handle other message types if needed
                        debug!("Received message from client: {:?}", msg);
                    }
                }
            }
            debug!("WebSocket receive task completed");
        });

        tokio::select! {
            _ = send_task => (),
            _ = recv_task => (),
        };

        info!("WebSocket connection closed");
    }

    /// Gracefully shuts down the notification hub
    pub async fn shutdown(&self) {
        info!("Shutting down NotificationHub");
        // Broadcast a final message if needed
        // Clean up resources
        // The broadcast channel will be automatically closed when the last sender is dropped
    }
}

impl<T> Default for NotificationHub<T>
where
    T: Clone + Send + Sync + std::fmt::Debug + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entities::inventory::Model as InventoryItem;
    use std::time::Duration;
    use tokio::sync::broadcast::error::TryRecvError;

    fn create_test_item(id: i32) -> InventoryItem {
        use chrono::Utc;
        use sea_orm::prelude::*;

        InventoryItem {
            id,
            name: format!("Test Item {}", id),
            description: "Test Description".to_string(),
            sku: format!("SKU{}", id),
            quantity: 10,
            price: Decimal::new(1000, 2), // $10.00
            category: "Test Category".to_string(),
            search_vector: "".to_string(),
            created_at: Utc::now().into(),
            updated_at: Utc::now().into(),
        }
    }

    #[tokio::test]
    async fn test_notification_hub_creation() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();
        assert_eq!(hub.subscriber_count(), 0);

        let stats = hub.get_stats().await;
        assert_eq!(stats.messages_sent.load(Ordering::Relaxed), 0);
        assert_eq!(stats.messages_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(stats.active_subscribers.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_custom_config() {
        let config = NotificationConfig {
            channel_size: 50,
            ws_timeout: Duration::from_secs(5),
        };
        let hub: NotificationHub<InventoryItem> = NotificationHub::with_config(config);
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn test_event_broadcasting() {
        let hub = NotificationHub::new();
        let mut receiver1 = hub.subscribe();
        let mut receiver2 = hub.subscribe();

        assert_eq!(hub.subscriber_count(), 2);

        // Test Created event
        let item = create_test_item(1);
        hub.broadcast(Event::Created(item.clone())).await.unwrap();

        // Use timeout to prevent test from hanging
        let timeout = Duration::from_secs(1);

        let result1 = tokio::time::timeout(timeout, receiver1.recv()).await;
        assert!(result1.is_ok(), "Receiver 1 timed out");
        if let Ok(Ok(Event::Created(received_item))) = result1 {
            assert_eq!(received_item.id, 1);
            assert_eq!(received_item.name, "Test Item 1");
        } else {
            panic!("Failed to receive Created event on receiver 1");
        }

        let result2 = tokio::time::timeout(timeout, receiver2.recv()).await;
        assert!(result2.is_ok(), "Receiver 2 timed out");
        if let Ok(Ok(Event::Created(received_item))) = result2 {
            assert_eq!(received_item.id, 1);
            assert_eq!(received_item.name, "Test Item 1");
        } else {
            panic!("Failed to receive Created event on receiver 2");
        }

        // Verify stats
        let stats = hub.get_stats().await;
        assert_eq!(stats.messages_sent.load(Ordering::Relaxed), 1);
        assert_eq!(stats.messages_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(stats.active_subscribers.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_multiple_events() {
        let hub = NotificationHub::new();
        let mut receiver = hub.subscribe();
        let timeout = Duration::from_secs(1);

        // Send multiple events
        let item1 = create_test_item(1);
        let item2 = create_test_item(2);

        // Test all event types
        let events = vec![
            Event::Created(item1.clone()),
            Event::Updated(item2.clone()),
            Event::Deleted(1),
            Event::Custom {
                event_type: "test".to_string(),
                payload: item1.clone(),
            },
        ];

        for event in events.clone() {
            hub.broadcast(event).await.unwrap();
        }

        // Verify events are received in order
        for expected_event in events {
            let result = tokio::time::timeout(timeout, receiver.recv()).await;
            assert!(result.is_ok(), "Receiver timed out");
            if let Ok(Ok(received_event)) = result {
                assert_eq!(received_event, expected_event);
            } else {
                panic!("Failed to receive expected event");
            }
        }

        // Verify stats
        let stats = hub.get_stats().await;
        assert_eq!(stats.messages_sent.load(Ordering::Relaxed), 4);
        assert_eq!(stats.messages_dropped.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_receiver_after_sender_dropped() {
        let hub = NotificationHub::new();
        let sender = hub.sender();
        let mut receiver = hub.subscribe();

        // Create a test event before dropping the sender
        let item = create_test_item(1);
        hub.broadcast(Event::Created(item.clone())).await.unwrap();

        // First receive the message
        match receiver.try_recv() {
            Ok(event) => {
                if let Event::Created(received_item) = event {
                    assert_eq!(received_item.id, item.id);
                } else {
                    panic!("Expected Created event");
                }
            }
            Err(_) => panic!("Expected to receive the message"),
        }

        // Drop the original sender
        drop(sender);

        // Now the channel should be closed or empty
        match receiver.try_recv() {
            Err(TryRecvError::Closed) | Err(TryRecvError::Empty) => (), // Expected
            _ => panic!("Expected channel to be closed or empty"),
        }
    }

    #[tokio::test]
    async fn test_late_subscriber() {
        let hub = NotificationHub::new();
        let timeout = Duration::from_secs(1);

        // Create a subscriber to keep the channel alive
        let _keep_alive = hub.subscribe();

        // Send an event before subscribing
        let item1 = create_test_item(1);
        hub.broadcast(Event::Created(item1)).await.unwrap();

        // Subscribe after the event
        let mut late_receiver = hub.subscribe();

        // Late subscriber should not receive the earlier event
        let item2 = create_test_item(2);
        let expected_event = Event::Created(item2.clone());
        hub.broadcast(expected_event.clone()).await.unwrap();

        let result = tokio::time::timeout(timeout, late_receiver.recv()).await;
        assert!(result.is_ok(), "Receiver timed out");
        if let Ok(Ok(received_event)) = result {
            assert_eq!(received_event, expected_event);
        } else {
            panic!("Failed to receive expected event");
        }

        // Verify stats
        let stats = hub.get_stats().await;
        assert_eq!(stats.messages_sent.load(Ordering::Relaxed), 2);
        assert_eq!(stats.messages_dropped.load(Ordering::Relaxed), 0);
        assert_eq!(stats.active_subscribers.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn test_broadcast_error() {
        let hub = NotificationHub::<InventoryItem>::new();
        let item = create_test_item(1);

        // No subscribers, should return error
        let result = hub.broadcast(Event::Created(item)).await;
        assert!(matches!(result, Err(NotificationError::SendError(_))));

        // Verify stats
        let stats = hub.get_stats().await;
        assert_eq!(stats.messages_sent.load(Ordering::Relaxed), 0);
        assert_eq!(stats.messages_dropped.load(Ordering::Relaxed), 1);
        assert_eq!(stats.active_subscribers.load(Ordering::Relaxed), 0);
    }

    // Test with a different type to demonstrate generics
    #[derive(Clone, Debug, PartialEq, Serialize)]
    struct TestUser {
        id: i32,
        name: String,
    }

    #[tokio::test]
    async fn test_different_type() {
        let hub: NotificationHub<TestUser> = NotificationHub::new();
        let mut receiver = hub.subscribe();

        let user = TestUser {
            id: 1,
            name: "Test User".to_string(),
        };

        hub.broadcast(Event::Created(user.clone())).await.unwrap();

        let timeout = Duration::from_secs(1);
        let result = tokio::time::timeout(timeout, receiver.recv()).await;

        assert!(result.is_ok(), "Receiver timed out");
        if let Ok(Ok(Event::Created(received_user))) = result {
            assert_eq!(received_user.id, user.id);
            assert_eq!(received_user.name, user.name);
        } else {
            panic!("Failed to receive Created event");
        }

        // Test custom event
        let custom_event = Event::Custom {
            event_type: "login".to_string(),
            payload: user.clone(),
        };
        hub.broadcast(custom_event.clone()).await.unwrap();

        let result = tokio::time::timeout(timeout, receiver.recv()).await;
        assert!(result.is_ok(), "Receiver timed out");
        if let Ok(Ok(received_event)) = result {
            assert_eq!(received_event, custom_event);
        } else {
            panic!("Failed to receive Custom event");
        }
    }
}
