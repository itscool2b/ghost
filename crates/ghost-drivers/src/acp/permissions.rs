//! Pending permission requests: minted ids, the turn each belongs to, `cancelled` answers at
//! interrupt and turn end, and withdrawal through `$/cancel_request`.
//!
//! Kept in arrival order, so answers that go out together (a turn's end, a shutdown) leave in the
//! order the agent asked, and replays compare exactly.

use serde_json::Value;

/// One request the agent waits on.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The JSON-RPC id the agent used, echoed in the answer.
    pub rpc_id: Value,
    /// The turn it was read in (rule 8); `None` outside a turn and before `Ready`.
    pub turn: Option<String>,
    pub option_ids: Vec<String>,
}

/// Every permission request not answered yet.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PendingPermissions {
    requests: Vec<(String, Pending)>,
}

impl PendingPermissions {
    pub fn insert(&mut self, request_id: String, pending: Pending) {
        self.requests.push((request_id, pending));
    }

    /// The request `request_id`, now answered; `None` when it is not pending (withdrawn, answered).
    pub fn take(&mut self, request_id: &str) -> Option<Pending> {
        let index = self.requests.iter().position(|(id, _)| id == request_id)?;
        Some(self.requests.remove(index).1)
    }

    /// The request the agent asked with `rpc_id` (`$/cancel_request` names it so).
    pub fn take_rpc(&mut self, rpc_id: &Value) -> Option<(String, Pending)> {
        let index = self
            .requests
            .iter()
            .position(|(_, pending)| pending.rpc_id == *rpc_id)?;
        Some(self.requests.remove(index))
    }

    /// Every request read in `turn`, in arrival order.
    pub fn take_turn(&mut self, turn: &str) -> Vec<(String, Pending)> {
        self.take_where(|pending| pending.turn.as_deref() == Some(turn))
    }

    /// Every request, in arrival order.
    pub fn take_all(&mut self) -> Vec<(String, Pending)> {
        std::mem::take(&mut self.requests)
    }

    pub fn len(&self) -> usize {
        self.requests.len()
    }

    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }

    fn take_where(&mut self, keep: impl Fn(&Pending) -> bool) -> Vec<(String, Pending)> {
        let (taken, rest) = std::mem::take(&mut self.requests)
            .into_iter()
            .partition(|(_, pending)| keep(pending));
        self.requests = rest;
        taken
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn pending(rpc_id: Value, turn: Option<&str>) -> Pending {
        Pending {
            rpc_id,
            turn: turn.map(str::to_owned),
            option_ids: vec!["allow".into()],
        }
    }

    #[test]
    fn requests_are_answered_once_by_id_rpc_id_or_turn() {
        let mut requests = PendingPermissions::default();
        requests.insert("T.p1".into(), pending(json!(5), Some("T")));
        requests.insert("S.p1".into(), pending(json!("bg-1"), None));
        requests.insert("T.p2".into(), pending(json!(6), Some("T")));
        requests.insert("U.p1".into(), pending(json!(7), Some("U")));
        assert_eq!(requests.len(), 4);

        assert_eq!(requests.take("T.p1").map(|p| p.rpc_id), Some(json!(5)));
        assert_eq!(requests.take("T.p1"), None, "an answer goes out once");

        let (id, withdrawn) = requests.take_rpc(&json!("bg-1")).expect("pending");
        assert_eq!((id.as_str(), withdrawn.turn), ("S.p1", None));
        assert_eq!(requests.take_rpc(&json!("bg-1")), None);
        assert_eq!(requests.take_rpc(&json!(99)), None, "other ids are ignored");

        let turn: Vec<String> = requests
            .take_turn("T")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(turn, ["T.p2"]);
        let rest: Vec<String> = requests.take_all().into_iter().map(|(id, _)| id).collect();
        assert_eq!(rest, ["U.p1"]);
        assert!(requests.is_empty());
    }

    #[test]
    fn numeric_and_string_rpc_ids_differ() {
        let mut requests = PendingPermissions::default();
        requests.insert("p".into(), pending(json!(1), None));
        assert_eq!(requests.take_rpc(&json!("1")), None);
        assert!(requests.take_rpc(&json!(1)).is_some());
    }
}
