//! Staying registered with the AT-SPI registry (`at-spi2-registryd`): the app
//! embeds its root once connected, and again whenever the registry's name
//! gets a new owner (the registry was restarted, or started on demand after
//! it went away), but not twice with the same registry, which would list the
//! app twice.

use super::REGISTRY;
use super::marshal::Args;
use crate::platform::a11y::Message;

/// The match rule for the registry's name changing owner.
pub(super) const OWNER_RULE: &str = "type='signal',sender='org.freedesktop.DBus',\
     interface='org.freedesktop.DBus',member='NameOwnerChanged',\
     arg0='org.a11y.atspi.Registry'";

/// The registry's new owner, if `m` says its name changed hands: the
/// owner's unique name, or empty when the registry went away.
pub(super) fn owner_change(m: &Message) -> Option<String> {
    if m.mtype != 4 || m.interface != "org.freedesktop.DBus" || m.member != "NameOwnerChanged" {
        return None;
    }
    let mut args = Args::new(&m.body);
    let (name, _old, new) = (args.str()?, args.str()?, args.str()?);
    (name == REGISTRY).then_some(new)
}

/// Where the app stands with the registry.
#[derive(Debug, Default)]
pub(super) struct Registration {
    /// The serial of the `Embed` call awaiting its answer.
    pending: Option<u32>,
    /// The registry's owner when that call went out (`None`: not known).
    sent_to: Option<String>,
    /// The registry's owner, as last announced.
    owner: Option<String>,
    /// The registry that embedded the app.
    with: Option<String>,
}

impl Registration {
    /// An `Embed` call went out as `serial`.
    pub(super) fn sent(&mut self, serial: u32) {
        self.pending = Some(serial);
        self.sent_to = self.owner.clone();
    }

    /// The registry's name changed hands (`new` empty: it went away):
    /// whether to embed now. Not while a call is on its way (it reaches the
    /// new owner, or fails and is sent again), nor with the registry that
    /// already embedded the app.
    pub(super) fn owner_changed(&mut self, new: &str) -> bool {
        self.owner = (!new.is_empty()).then(|| new.to_string());
        if new.is_empty() {
            self.with = None;
            return false;
        }
        self.pending.is_none() && self.with.as_deref() != Some(new)
    }

    /// The answer to a call from `sender` (`ok`: a return, else an error):
    /// `None` if it isn't the pending `Embed`'s, else whether to embed again
    /// (the registry changed hands since the call went out, and the answer
    /// came from the one that's gone, or the call failed).
    pub(super) fn answered(&mut self, serial: u32, sender: &str, ok: bool) -> Option<bool> {
        if self.pending != Some(serial) {
            return None;
        }
        self.pending = None;
        let changed = self.owner != self.sent_to;
        if ok {
            self.with = Some(sender.to_string());
        }
        Some(match self.owner.as_deref() {
            // Embedded by a registry that has since gone, or failed because
            // the one it was sent to went.
            Some(owner) if ok => sender != owner,
            Some(_) => changed,
            None => false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::atspi::marshal::Body;

    fn owner_changed(name: &str, old: &str, new: &str) -> Message {
        let mut body = Body::new();
        body.str(name).str(old).str(new);
        Message {
            mtype: 4,
            interface: "org.freedesktop.DBus".into(),
            member: "NameOwnerChanged".into(),
            body: body.0,
            ..Default::default()
        }
    }

    #[test]
    fn reads_the_registry_changing_hands() {
        assert_eq!(
            owner_change(&owner_changed(REGISTRY, "", ":1.9")).as_deref(),
            Some(":1.9")
        );
        assert_eq!(
            owner_change(&owner_changed(REGISTRY, ":1.3", "")).as_deref(),
            Some("")
        );
        assert_eq!(
            owner_change(&owner_changed("org.a11y.Other", "", ":1.9")),
            None
        );
        let mut call = owner_changed(REGISTRY, "", ":1.9");
        call.mtype = 1;
        assert_eq!(owner_change(&call), None);
    }

    #[test]
    fn a_registry_started_by_the_first_embed_embeds_once() {
        // Embed goes out with no registry running; the bus starts one, which
        // takes the name, then answers.
        let mut r = Registration::default();
        r.sent(5);
        assert!(!r.owner_changed(":1.1"), "the call on its way reaches it");
        assert_eq!(r.answered(5, ":1.1", true), Some(false));
        // The same registry announced again: nothing to do.
        assert!(!r.owner_changed(":1.1"));
        assert_eq!(r.answered(9, ":1.1", true), None, "not our call");
    }

    #[test]
    fn a_restarted_registry_embeds_again() {
        let mut r = Registration::default();
        r.sent(5);
        assert_eq!(r.answered(5, ":1.1", true), Some(false));
        assert!(!r.owner_changed(""), "gone: nothing to embed with");
        assert!(r.owner_changed(":1.4"), "back: embed again");
        r.sent(6);
        assert_eq!(r.answered(6, ":1.4", true), Some(false));
    }

    #[test]
    fn a_registry_that_left_while_the_call_was_on_its_way_is_asked_again() {
        let mut r = Registration::default();
        assert!(r.owner_changed(":1.1"));
        r.sent(5);
        assert!(!r.owner_changed(""));
        assert!(!r.owner_changed(":1.4"), "the call is still on its way");
        // It failed (the old registry left without answering): send again.
        assert_eq!(r.answered(5, ":1.1", false), Some(true));
        r.sent(6);
        assert_eq!(r.answered(6, ":1.4", true), Some(false));
        // A refusal from the registry it was sent to isn't retried.
        assert!(!r.owner_changed(":1.4"));
        let mut r = Registration::default();
        assert!(r.owner_changed(":1.1"));
        r.sent(7);
        assert_eq!(r.answered(7, ":1.1", false), Some(false));
    }
}
