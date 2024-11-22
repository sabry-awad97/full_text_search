use axum::extract::ws::{Message, WebSocket};
use chrono::{DateTime, Utc};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
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
/// Default number of messages to keep in history
const DEFAULT_HISTORY_SIZE: usize = 100;

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
    #[error("Invalid topic: {0}")]
    InvalidTopic(String),
}

/// Message priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Represents different types of events that can be broadcasted
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Event<T> {
    pub payload: EventPayload<T>,
    pub topic: Option<String>,
    pub priority: Priority,
    pub timestamp: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum EventPayload<T> {
    Created(T),
    Updated(T),
    Deleted(i32),
    /// Custom event type for application-specific events
    Custom {
        event_type: String,
        payload: T,
    },
    /// Batch events for efficient processing
    Batch(Vec<T>),
}

impl<T> Event<T> {
    pub fn new(payload: EventPayload<T>) -> Self {
        Self {
            payload,
            topic: None,
            priority: Priority::default(),
            timestamp: Utc::now(),
        }
    }

    pub fn with_topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }
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
    history_size: usize,
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            channel_size: DEFAULT_CHANNEL_SIZE,
            ws_timeout: Duration::from_secs(DEFAULT_WS_TIMEOUT_SECS),
            history_size: DEFAULT_HISTORY_SIZE,
        }
    }
}

/// Subscription filter for receiving specific events
#[derive(Clone, Debug)]
pub struct SubscriptionFilter {
    topics: Option<Vec<String>>,
    min_priority: Priority,
}

impl Default for SubscriptionFilter {
    fn default() -> Self {
        Self {
            topics: None,
            min_priority: Priority::Low,
        }
    }
}

impl SubscriptionFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_topics(mut self, topics: Vec<String>) -> Self {
        self.topics = Some(topics);
        self
    }

    pub fn with_min_priority(mut self, priority: Priority) -> Self {
        self.min_priority = priority;
        self
    }

    fn matches(&self, event: &Event<impl Clone>) -> bool {
        if event.priority < self.min_priority {
            return false;
        }

        if let Some(topics) = &self.topics {
            if let Some(event_topic) = &event.topic {
                return topics.iter().any(|t| t == event_topic);
            }
            return false;
        }

        true
    }
}

/// A hub for managing real-time notifications and WebSocket connections
#[derive(Clone)]
pub struct NotificationHub<T> {
    sender: Sender<Event<T>>,
    stats: Arc<NotificationStats>,
    #[allow(dead_code)]
    config: NotificationConfig,
    message_history: Arc<RwLock<VecDeque<Event<T>>>>,
    topic_subscribers: Arc<RwLock<HashMap<String, usize>>>,
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
            config: config.clone(),
            message_history: Arc::new(RwLock::new(VecDeque::with_capacity(config.history_size))),
            topic_subscribers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Returns a clone of the sender
    pub fn sender(&self) -> Sender<Event<T>> {
        self.sender.clone()
    }

    /// Creates a new subscriber with optional filter
    pub fn subscribe_with_filter(&self, filter: SubscriptionFilter) -> Receiver<Event<T>> {
        self.stats.record_subscriber_added();
        let (tx, rx) = broadcast::channel(self.config.channel_size);
        let filter = Arc::new(filter);
        let sender_clone = self.sender.clone();
        let filter_clone = filter.clone();

        // Update topic subscriber count
        if let Some(topics) = &filter.topics {
            if let Ok(mut topic_subs) = self.topic_subscribers.try_write() {
                for topic in topics {
                    *topic_subs.entry(topic.clone()).or_default() += 1;
                }
            }
        }

        // Send historical messages that match the filter
        if let Ok(history) = self.message_history.try_read() {
            for event in history.iter() {
                if filter.matches(event) {
                    let _ = tx.send(event.clone());
                }
            }
        }

        // Spawn task to filter and forward messages
        tokio::spawn(async move {
            let mut receiver = sender_clone.subscribe();
            while let Ok(event) = receiver.recv().await {
                if filter_clone.matches(&event) {
                    let _ = tx.send(event);
                }
            }
        });

        rx
    }

    /// Creates a new subscriber
    pub fn subscribe(&self) -> Receiver<Event<T>> {
        self.stats.record_subscriber_added();
        let receiver = self.sender.subscribe();

        // Send historical messages to new subscriber
        if let Ok(history) = self.message_history.try_read() {
            for event in history.iter() {
                let _ = self.sender.send(event.clone());
            }
        }

        receiver
    }

    /// Broadcasts an event to all subscribers
    pub async fn broadcast(&self, event: Event<T>) -> Result<(), NotificationError> {
        // Store in history first
        {
            let mut history = self.message_history.write().await;
            if history.len() >= self.config.history_size {
                history.pop_front();
            }
            history.push_back(event.clone());
        }

        // Ensure event is sent to all subscribers
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

    /// Broadcasts multiple events as a batch
    pub async fn broadcast_batch(&self, events: Vec<T>) -> Result<(), NotificationError> {
        if events.is_empty() {
            return Ok(());
        }

        let batch_event = Event::new(EventPayload::Batch(events));
        self.broadcast(batch_event).await
    }

    /// Returns the number of subscribers for a specific topic
    pub async fn topic_subscriber_count(&self, topic: &str) -> usize {
        self.topic_subscribers
            .read()
            .await
            .get(topic)
            .copied()
            .unwrap_or(0)
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

    /// Returns the message history
    pub async fn get_history(&self) -> Vec<Event<T>> {
        self.message_history.read().await.iter().cloned().collect()
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
    use tokio::time::Duration;

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
    }

    #[tokio::test]
    async fn test_custom_config() {
        let config = NotificationConfig {
            channel_size: 50,
            ws_timeout: Duration::from_secs(5),
            history_size: 50,
        };
        let hub: NotificationHub<InventoryItem> = NotificationHub::with_config(config);
        assert_eq!(hub.subscriber_count(), 0);
    }

    #[tokio::test]
    async fn test_topic_filtering() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();

        // Create filters for different topics
        let inventory_filter = SubscriptionFilter::new().with_topics(vec!["inventory".to_string()]);
        let sales_filter = SubscriptionFilter::new().with_topics(vec!["sales".to_string()]);
        let multi_filter = SubscriptionFilter::new()
            .with_topics(vec!["inventory".to_string(), "sales".to_string()]);

        // Create receivers with filters
        let mut inventory_receiver = hub.subscribe_with_filter(inventory_filter);
        let mut sales_receiver = hub.subscribe_with_filter(sales_filter);
        let mut multi_receiver = hub.subscribe_with_filter(multi_filter);

        // Wait for initialization
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Clear any pending messages
        while inventory_receiver.try_recv().is_ok() {}
        while sales_receiver.try_recv().is_ok() {}
        while multi_receiver.try_recv().is_ok() {}

        // Send events with different topics
        let inventory_item = create_test_item(1);
        let sales_item = create_test_item(2);
        let inventory_event =
            Event::new(EventPayload::Created(inventory_item.clone())).with_topic("inventory");
        let sales_event = Event::new(EventPayload::Created(sales_item.clone())).with_topic("sales");

        // Broadcast events with delays
        hub.broadcast(inventory_event.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        hub.broadcast(sales_event.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let timeout = Duration::from_millis(200);

        // Inventory receiver should only get inventory event
        let result = tokio::time::timeout(timeout, inventory_receiver.recv()).await;
        assert!(result.is_ok(), "Inventory receiver timed out");
        let received = result.unwrap().unwrap();
        assert_eq!(
            received.topic.as_deref(),
            Some("inventory"),
            "Wrong topic received by inventory subscriber"
        );
        if let EventPayload::Created(item) = received.payload {
            assert_eq!(
                item.id, inventory_item.id,
                "Wrong item received by inventory subscriber"
            );
        } else {
            panic!("Wrong payload type received by inventory subscriber");
        }

        // Sales receiver should only get sales event
        let result = tokio::time::timeout(timeout, sales_receiver.recv()).await;
        assert!(result.is_ok(), "Sales receiver timed out");
        let received = result.unwrap().unwrap();
        assert_eq!(
            received.topic.as_deref(),
            Some("sales"),
            "Wrong topic received by sales subscriber"
        );
        if let EventPayload::Created(item) = received.payload {
            assert_eq!(
                item.id, sales_item.id,
                "Wrong item received by sales subscriber"
            );
        } else {
            panic!("Wrong payload type received by sales subscriber");
        }

        // Multi receiver should get both events
        let mut received_events = Vec::new();
        for _ in 0..2 {
            if let Ok(Ok(event)) = tokio::time::timeout(timeout, multi_receiver.recv()).await {
                received_events.push(event);
            }
        }
        assert_eq!(
            received_events.len(),
            2,
            "Multi subscriber didn't receive both events"
        );

        let received_topics: Vec<_> = received_events
            .iter()
            .filter_map(|e| e.topic.as_deref())
            .collect();
        assert!(
            received_topics.contains(&"inventory"),
            "Multi subscriber missing inventory event"
        );
        assert!(
            received_topics.contains(&"sales"),
            "Multi subscriber missing sales event"
        );

        let received_ids: Vec<_> = received_events
            .iter()
            .filter_map(|e| {
                if let EventPayload::Created(item) = &e.payload {
                    Some(item.id)
                } else {
                    None
                }
            })
            .collect();
        assert!(
            received_ids.contains(&inventory_item.id),
            "Multi subscriber missing inventory item"
        );
        assert!(
            received_ids.contains(&sales_item.id),
            "Multi subscriber missing sales item"
        );
    }

    #[tokio::test]
    async fn test_priority_filtering() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();

        // Create subscribers with different priority filters
        let high_filter = SubscriptionFilter::new().with_min_priority(Priority::High);
        let normal_filter = SubscriptionFilter::new().with_min_priority(Priority::Normal);

        // Subscribe and wait for initialization
        let mut high_receiver = hub.subscribe_with_filter(high_filter);
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut normal_receiver = hub.subscribe_with_filter(normal_filter);
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Clear any pending messages
        while high_receiver.try_recv().is_ok() {}
        while normal_receiver.try_recv().is_ok() {}

        // Send events with different priorities
        let items = (1..=4).map(create_test_item).collect::<Vec<_>>();
        let events = vec![
            Event::new(EventPayload::Created(items[0].clone())).with_priority(Priority::Low),
            Event::new(EventPayload::Created(items[1].clone())).with_priority(Priority::Normal),
            Event::new(EventPayload::Created(items[2].clone())).with_priority(Priority::High),
            Event::new(EventPayload::Created(items[3].clone())).with_priority(Priority::Critical),
        ];

        // Broadcast events with delays
        for event in &events {
            hub.broadcast(event.clone()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let timeout = Duration::from_millis(200);

        // High priority receiver should only get high and critical events
        let mut high_received = Vec::new();
        for _ in 0..2 {
            if let Ok(Ok(event)) = tokio::time::timeout(timeout, high_receiver.recv()).await {
                high_received.push((
                    event.priority,
                    if let EventPayload::Created(item) = event.payload {
                        item.id
                    } else {
                        panic!("Wrong payload type")
                    },
                ));
            }
        }
        assert_eq!(
            high_received.len(),
            2,
            "High priority subscriber didn't receive expected number of events"
        );
        assert!(
            high_received
                .iter()
                .any(|(p, id)| *p == Priority::High && *id == items[2].id),
            "Missing High priority event"
        );
        assert!(
            high_received
                .iter()
                .any(|(p, id)| *p == Priority::Critical && *id == items[3].id),
            "Missing Critical priority event"
        );

        // Normal priority receiver should get normal, high, and critical events
        let mut normal_received = Vec::new();
        for _ in 0..3 {
            if let Ok(Ok(event)) = tokio::time::timeout(timeout, normal_receiver.recv()).await {
                normal_received.push((
                    event.priority,
                    if let EventPayload::Created(item) = event.payload {
                        item.id
                    } else {
                        panic!("Wrong payload type")
                    },
                ));
            }
        }

        assert_eq!(
            normal_received.len(),
            3,
            "Normal priority subscriber didn't receive expected number of events"
        );
        assert!(
            normal_received
                .iter()
                .any(|(p, id)| *p == Priority::Normal && *id == items[1].id),
            "Missing Normal priority event"
        );
        assert!(
            normal_received
                .iter()
                .any(|(p, id)| *p == Priority::High && *id == items[2].id),
            "Missing High priority event"
        );
        assert!(
            normal_received
                .iter()
                .any(|(p, id)| *p == Priority::Critical && *id == items[3].id),
            "Missing Critical priority event"
        );
    }

    #[tokio::test]
    async fn test_message_history() {
        let config = NotificationConfig {
            channel_size: 50,
            ws_timeout: Duration::from_secs(5),
            history_size: 2,
        };
        let hub: NotificationHub<InventoryItem> = NotificationHub::with_config(config);

        // Subscribe and wait for initialization
        let mut initial_receiver = hub.subscribe();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Clear any pending messages
        while initial_receiver.try_recv().is_ok() {}

        // Send three events (history size is 2)
        let items = vec![
            Event::new(EventPayload::Created(create_test_item(1))),
            Event::new(EventPayload::Created(create_test_item(2))),
            Event::new(EventPayload::Created(create_test_item(3))),
        ];

        // Broadcast events with delays
        for event in &items {
            hub.broadcast(event.clone()).await.unwrap();
            // Consume the event from the initial receiver
            let _ = initial_receiver.try_recv();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Wait for history to be updated
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Check history size and contents
        let history = hub.get_history().await;
        assert_eq!(history.len(), 2, "History size doesn't match configuration");

        // Verify history contains the last two events
        let history_ids: Vec<_> = history
            .iter()
            .filter_map(|event| {
                if let EventPayload::Created(item) = &event.payload {
                    Some(item.id)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            history_ids,
            vec![2, 3],
            "History doesn't contain expected items"
        );

        // New subscriber should receive historical messages
        let mut receiver = hub.subscribe();
        tokio::time::sleep(Duration::from_millis(10)).await;
        let timeout = Duration::from_millis(200);

        // Collect received messages
        let mut received_ids = Vec::new();

        for _ in 0..2 {
            if let Ok(Ok(event)) = tokio::time::timeout(timeout, receiver.recv()).await {
                if let EventPayload::Created(item) = event.payload {
                    received_ids.push(item.id);
                }
            }
        }

        assert_eq!(
            received_ids.len(),
            2,
            "New subscriber didn't receive all historical events"
        );
        assert_eq!(
            received_ids,
            vec![2, 3],
            "New subscriber received wrong historical events"
        );
    }

    #[tokio::test]
    async fn test_batch_broadcasting() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();

        // Subscribe and wait for initialization
        let mut receiver = hub.subscribe();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Clear any pending messages
        while receiver.try_recv().is_ok() {}

        // Create batch of items with different IDs
        let items: Vec<_> = (1..=3).map(create_test_item).collect();

        // Broadcast batch
        hub.broadcast_batch(items.clone()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;

        let timeout = Duration::from_millis(200);
        let result = tokio::time::timeout(timeout, receiver.recv()).await;

        assert!(
            result.is_ok(),
            "Failed to receive batch event within timeout"
        );
        if let Ok(Ok(Event {
            payload: EventPayload::Batch(received_items),
            ..
        })) = result
        {
            assert_eq!(
                received_items.len(),
                3,
                "Batch event contains wrong number of items"
            );

            // Verify each item in the batch
            for (expected, received) in items.iter().zip(received_items.iter()) {
                assert_eq!(expected.id, received.id, "Mismatched item ID in batch");
                assert_eq!(
                    expected.name, received.name,
                    "Mismatched item name in batch"
                );
            }
        } else {
            panic!("Received wrong event type, expected Batch");
        }

        // Verify the batch is stored as a single event in history
        let history = hub.get_history().await;
        assert_eq!(
            history.len(),
            1,
            "Batch event should be stored as a single history entry"
        );

        if let EventPayload::Batch(history_items) = &history[0].payload {
            assert_eq!(
                history_items.len(),
                3,
                "History batch contains wrong number of items"
            );
        } else {
            panic!("History contains wrong event type, expected Batch");
        }
    }

    #[tokio::test]
    async fn test_topic_subscriber_count() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();

        // Create multiple subscribers for different topics
        let inventory_filter = SubscriptionFilter::new().with_topics(vec!["inventory".to_string()]);
        let sales_filter = SubscriptionFilter::new().with_topics(vec!["sales".to_string()]);
        let multi_filter = SubscriptionFilter::new()
            .with_topics(vec!["inventory".to_string(), "sales".to_string()]);

        // Subscribe with delays between subscriptions
        let _inventory_receiver = hub.subscribe_with_filter(inventory_filter.clone());
        tokio::time::sleep(Duration::from_millis(10)).await;
        let _sales_receiver = hub.subscribe_with_filter(sales_filter);
        tokio::time::sleep(Duration::from_millis(10)).await;
        let _multi_receiver = hub.subscribe_with_filter(multi_filter);
        tokio::time::sleep(Duration::from_millis(10)).await;
        let _inventory_receiver2 = hub.subscribe_with_filter(inventory_filter);
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Check subscriber counts
        assert_eq!(
            hub.topic_subscriber_count("inventory").await,
            3,
            "Wrong number of inventory subscribers (2 inventory + 1 multi)"
        );
        assert_eq!(
            hub.topic_subscriber_count("sales").await,
            2,
            "Wrong number of sales subscribers (1 sales + 1 multi)"
        );
        assert_eq!(
            hub.topic_subscriber_count("unknown").await,
            0,
            "Unknown topic should have 0 subscribers"
        );

        // Verify total subscriber count
        assert_eq!(
            hub.subscriber_count(),
            4,
            "Wrong total number of subscribers"
        );
    }

    #[tokio::test]
    async fn test_empty_batch() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();
        let mut receiver = hub.subscribe();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Clear any pending messages
        while receiver.try_recv().is_ok() {}

        // Test empty batch
        assert!(
            hub.broadcast_batch(vec![]).await.is_ok(),
            "Empty batch should be handled gracefully"
        );

        // Verify no message was sent
        let timeout = Duration::from_millis(50);
        let result = tokio::time::timeout(timeout, receiver.recv()).await;
        assert!(
            result.is_err(),
            "Should not receive any message for empty batch"
        );
    }

    #[tokio::test]
    async fn test_subscriber_cleanup() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();

        // Create a scope to drop the receiver
        {
            let _receiver = hub.subscribe();
            assert_eq!(hub.subscriber_count(), 1, "Should have one subscriber");
        }

        // Wait for cleanup
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            hub.subscriber_count(),
            0,
            "Subscriber should be cleaned up after drop"
        );
    }

    #[tokio::test]
    async fn test_multiple_events_ordering() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();
        let mut receiver = hub.subscribe();
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Clear any pending messages
        while receiver.try_recv().is_ok() {}

        // Send multiple events
        let events: Vec<_> = (1..=3)
            .map(|id| Event::new(EventPayload::Created(create_test_item(id))))
            .collect();

        // Broadcast events with delays
        for event in &events {
            hub.broadcast(event.clone()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Verify events are received in order
        let timeout = Duration::from_millis(200);
        let mut received_ids = Vec::new();

        for _ in 0..3 {
            if let Ok(Ok(event)) = tokio::time::timeout(timeout, receiver.recv()).await {
                if let EventPayload::Created(item) = event.payload {
                    received_ids.push(item.id);
                }
            }
        }

        assert_eq!(
            received_ids,
            vec![1, 2, 3],
            "Events not received in correct order"
        );
    }

    #[tokio::test]
    async fn test_mixed_priority_and_topic() {
        let hub: NotificationHub<InventoryItem> = NotificationHub::new();

        // Create a filter with both topic and priority
        let filter = SubscriptionFilter::new()
            .with_topics(vec!["inventory".to_string()])
            .with_min_priority(Priority::High);

        let mut receiver = hub.subscribe_with_filter(filter);
        tokio::time::sleep(Duration::from_millis(10)).await;

        // Clear any pending messages
        while receiver.try_recv().is_ok() {}

        // Create events with different combinations
        let events = vec![
            Event::new(EventPayload::Created(create_test_item(1)))
                .with_topic("inventory")
                .with_priority(Priority::Low),
            Event::new(EventPayload::Created(create_test_item(2)))
                .with_topic("sales")
                .with_priority(Priority::Critical),
            Event::new(EventPayload::Created(create_test_item(3)))
                .with_topic("inventory")
                .with_priority(Priority::High),
        ];

        // Broadcast events
        for event in &events {
            hub.broadcast(event.clone()).await.unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        // Should only receive the high priority inventory event
        let timeout = Duration::from_millis(100);
        let result = tokio::time::timeout(timeout, receiver.recv()).await;

        assert!(result.is_ok(), "Failed to receive event within timeout");
        if let Ok(Ok(event)) = result {
            if let EventPayload::Created(item) = event.payload {
                assert_eq!(item.id, 3, "Received wrong event");
                assert_eq!(event.topic.as_deref(), Some("inventory"), "Wrong topic");
                assert_eq!(event.priority, Priority::High, "Wrong priority");
            } else {
                panic!("Wrong payload type");
            }
        }

        // Verify no more events are received
        assert!(
            tokio::time::timeout(timeout, receiver.recv())
                .await
                .is_err(),
            "Should not receive any more events"
        );
    }
}
