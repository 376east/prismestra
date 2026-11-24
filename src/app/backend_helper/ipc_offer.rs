use super::{Identity, Request, clock, live_request};
use serde::{Deserialize, Serialize};
// Separate record in the protected outbox. Older Status readers keep their
// existing strict schema. The caller must read a protected, bounded record.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Offer {
    pub(super) nonce: String,
    pub(super) owner: Identity,
    pub(super) helper: Identity,
    pub(super) epoch: u64,
    pub(super) owner_tick: u64,
}

impl Offer {
    // Both Identity values come from retained live process handles and the
    // endpoint PID obtained from the kernel. A frame cannot supply either.
    pub(super) fn admit(
        &self,
        request: &Request,
        live_owner: &Identity,
        live_helper: &Identity,
        epoch: u64,
        now: u64,
    ) -> Option<u64> {
        if self.epoch == 0
            || self.epoch != epoch
            || self.nonce != request.nonce
            || self.owner != request.parent
            || &self.owner != live_owner
            || &self.helper != live_helper
            || self.helper.executable != self.owner.executable
            || self.helper.pid == self.owner.pid
            || self.owner_tick > request.timestamp_ms
            || !request.restarting
            || !live_request(request, now)
        {
            return None;
        }
        let deadline = clock::deadline(self.owner_tick, now).ok()?;
        (now < deadline).then_some(deadline)
    }
}
