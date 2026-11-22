//! One-time WebSocket admission tickets (the chest ws-bridge pattern).
//!
//! The browser's JSON-RPC client only ATTEMPTS the WebSocket upgrade
//! when its token getter answers non-null — and a long-lived session
//! JWT must never ride a URL (logs, history, proxies). The ticket
//! reconciles both: a short-lived, single-use, user-bound random
//! credential whose only job is to open the socket. The browser's
//! PRIMARY channel stays the httpOnly cookie (which same-origin WS
//! upgrades also carry); the ticket is what the `?token=` URL slot
//! holds, and it dies the moment it is used.
//!
//! SINGLE-INSTANCE SCOPE: the store is process memory. A multi-replica
//! deployment must either shard by sticky routing (the mint and the
//! upgrade hit the same replica) or back the store with the shared
//! database — neither applies to this wave's single-service deployment
//! (see the compose shape), so the in-memory table is honest.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use uuid::Uuid;

/// How long a minted ticket may live before its upgrade.
pub const TICKET_TTL: Duration = Duration::from_secs(30);

/// One live ticket.
struct TicketEntry {
    /// The user the ticket admits.
    user_id: Uuid,
    /// Mint time — diagnostic provenance (age of an unconsumed ticket);
    /// kept for the debugging story, not read on the hot path.
    #[allow(dead_code)]
    minted_at: Instant,
    /// Expiry.
    expires_at: Instant,
}

/// The in-memory single-use ticket table.
#[derive(Default)]
pub struct WsTicketStore {
    entries: Mutex<HashMap<String, TicketEntry>>,
}

impl WsTicketStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a ticket bound to `user_id`, pruning expired entries on the
    /// way (the table is tiny; opportunistic pruning is enough).
    pub fn mint(&self, user_id: Uuid) -> String {
        // A v4 UUID carries 122 random bits — collision- and guessing-
        // proof for a 30 s single-use credential.
        let ticket = Uuid::new_v4().simple().to_string();
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("ticket table lock");
        entries.retain(|_, entry| entry.expires_at > now);
        entries.insert(
            ticket.clone(),
            TicketEntry {
                user_id,
                minted_at: now,
                expires_at: now + TICKET_TTL,
            },
        );
        ticket
    }

    /// Consume a ticket: valid and unexpired → the bound user (the
    /// ticket is REMOVED — single use); anything else → `None`.
    pub fn consume(&self, ticket: &str) -> Option<Uuid> {
        let now = Instant::now();
        let mut entries = self.entries.lock().expect("ticket table lock");
        match entries.remove(ticket) {
            Some(entry) if entry.expires_at > now => Some(entry.user_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ticket_admits_exactly_once() {
        let store = WsTicketStore::new();
        let user = Uuid::new_v4();
        let ticket = store.mint(user);
        assert_eq!(store.consume(&ticket), Some(user));
        assert_eq!(store.consume(&ticket), None, "single use, no replay");
    }

    #[test]
    fn an_unknown_ticket_is_refused() {
        let store = WsTicketStore::new();
        assert_eq!(store.consume("not-a-ticket"), None);
    }

    #[test]
    fn expired_tickets_are_pruned_and_refused() {
        let store = WsTicketStore::new();
        let user = Uuid::new_v4();
        // Mint with an already-elapsed expiry by direct insertion.
        let ticket = "deadbeefdeadbeefdeadbeefdeadbeef".to_owned();
        store.entries.lock().unwrap().insert(
            ticket.clone(),
            TicketEntry {
                user_id: user,
                minted_at: Instant::now() - TICKET_TTL - Duration::from_secs(1),
                expires_at: Instant::now() - Duration::from_secs(1),
            },
        );
        assert_eq!(store.consume(&ticket), None, "expired tickets never admit");
    }
}
