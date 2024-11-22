use crate::entities::inventory::Model as InventoryItem;
use axum::extract::ws::{Message, WebSocket};
use futures::{SinkExt, StreamExt};
use serde::Serialize;
use tokio::sync::broadcast::{self, Sender};
use tokio_stream::wrappers::BroadcastStream;

#[derive(Clone, Debug, Serialize, PartialEq)]
pub enum InventoryEvent {
    Created(InventoryItem),
    Updated(InventoryItem),
    Deleted(i32),
}

pub struct NotificationHub {
    sender: Sender<InventoryEvent>,
}

impl NotificationHub {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(100);
        Self { sender }
    }

    pub fn sender(&self) -> Sender<InventoryEvent> {
        self.sender.clone()
    }

    pub async fn handle_socket(socket: WebSocket, sender: Sender<InventoryEvent>) {
        let (mut sender_ws, mut receiver) = socket.split();
        let mut receiver_stream = BroadcastStream::new(sender.subscribe());

        let mut send_task = tokio::spawn(async move {
            while let Some(Ok(msg)) = receiver_stream.next().await {
                if let Ok(json) = serde_json::to_string(&msg) {
                    if sender_ws.send(Message::Text(json)).await.is_err() {
                        break;
                    }
                }
            }
        });

        let mut recv_task = tokio::spawn(async move {
            while let Some(Ok(_)) = receiver.next().await {
                // Handle client messages if needed
            }
        });

        // If either task completes, cancel both
        tokio::select! {
            _ = &mut send_task => recv_task.abort(),
            _ = &mut recv_task => send_task.abort(),
        }
    }
}

impl Default for NotificationHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let hub = NotificationHub::new();
        assert!(hub.sender.receiver_count() == 0);
    }

    #[tokio::test]
    async fn test_event_broadcasting() {
        let hub = NotificationHub::new();
        let sender = hub.sender();
        let mut receiver1 = sender.subscribe();
        let mut receiver2 = sender.subscribe();

        // Test Created event
        let item = create_test_item(1);
        sender.send(InventoryEvent::Created(item.clone())).unwrap();

        // Use timeout to prevent test from hanging
        let timeout = tokio::time::Duration::from_secs(1);

        let result1 = tokio::time::timeout(timeout, receiver1.recv()).await;
        assert!(result1.is_ok(), "Receiver 1 timed out");
        if let Ok(Ok(InventoryEvent::Created(received_item))) = result1 {
            assert_eq!(received_item.id, 1);
            assert_eq!(received_item.name, "Test Item 1");
        } else {
            panic!("Failed to receive Created event on receiver 1");
        }

        let result2 = tokio::time::timeout(timeout, receiver2.recv()).await;
        assert!(result2.is_ok(), "Receiver 2 timed out");
        if let Ok(Ok(InventoryEvent::Created(received_item))) = result2 {
            assert_eq!(received_item.id, 1);
            assert_eq!(received_item.name, "Test Item 1");
        } else {
            panic!("Failed to receive Created event on receiver 2");
        }
    }

    #[tokio::test]
    async fn test_multiple_events() {
        let hub = NotificationHub::new();
        let sender = hub.sender();
        let mut receiver = sender.subscribe();
        let timeout = tokio::time::Duration::from_secs(1);

        // Send multiple events
        let item1 = create_test_item(1);
        let item2 = create_test_item(2);

        sender.send(InventoryEvent::Created(item1.clone())).unwrap();
        sender.send(InventoryEvent::Updated(item2.clone())).unwrap();
        sender.send(InventoryEvent::Deleted(1)).unwrap();

        // Verify events are received in order
        let events = vec![
            InventoryEvent::Created(item1),
            InventoryEvent::Updated(item2),
            InventoryEvent::Deleted(1),
        ];

        for expected_event in events {
            let result = tokio::time::timeout(timeout, receiver.recv()).await;
            assert!(result.is_ok(), "Receiver timed out");
            if let Ok(Ok(received_event)) = result {
                assert_eq!(received_event, expected_event);
            } else {
                panic!("Failed to receive expected event");
            }
        }
    }

    #[tokio::test]
    async fn test_receiver_after_sender_dropped() {
        let hub = NotificationHub::new();
        let sender = hub.sender();
        let mut receiver = sender.subscribe();

        // Create a test event before dropping the sender
        let item = create_test_item(1);
        sender.send(InventoryEvent::Created(item.clone())).unwrap();

        // First receive the message
        match receiver.try_recv() {
            Ok(event) => {
                if let InventoryEvent::Created(received_item) = event {
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
        let sender = hub.sender();
        let timeout = tokio::time::Duration::from_secs(1);

        // Create a subscriber to keep the channel alive
        let _keep_alive = sender.subscribe();

        // Send an event before subscribing
        let item1 = create_test_item(1);
        sender.send(InventoryEvent::Created(item1)).unwrap();

        // Subscribe after the event
        let mut late_receiver = sender.subscribe();

        // Late subscriber should not receive the earlier event
        let item2 = create_test_item(2);
        let expected_event = InventoryEvent::Created(item2.clone());
        sender.send(expected_event.clone()).unwrap();

        let result = tokio::time::timeout(timeout, late_receiver.recv()).await;
        assert!(result.is_ok(), "Receiver timed out");
        if let Ok(Ok(received_event)) = result {
            assert_eq!(received_event, expected_event);
        } else {
            panic!("Failed to receive expected event");
        }
    }
}
