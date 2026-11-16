//! What the user chose to keep, told to the network actor (which tells
//! Reticulum, see [`crate::net::pins`]): who isn't forgotten when a flood
//! of announces from strangers fills Reticulum's tables.

use super::App;
use crate::net::{Hash, NetCommand, parse_hash};
use crate::store::Trust;

impl App {
    /// The destinations the user has chosen, most important first: contacts
    /// (trusted, or left as is), the propagation node, RRC hubs, saved
    /// NomadNet nodes (pages saved, nodes identified to) and rngit nodes.
    /// Everything here comes from what is saved; nothing a peer sent can add
    /// to it. [`crate::net::pins`] cuts the list at its limit.
    fn chosen_destinations(&self) -> Vec<Hash> {
        let contacts = self
            .store
            .contacts
            .iter()
            .filter(|(_, c)| matches!(c.trust, Trust::Trusted | Trust::Untrusted))
            .filter_map(|(k, _)| parse_hash(k));
        let propagation = self.propagation_node();
        let hubs = self.store.rrc_hubs.iter().filter_map(|h| parse_hash(&h.hash));
        let pages = self.store.saved.iter().filter_map(|b| parse_hash(b.url.split(':').next()?));
        let nodes = self.store.identified_nodes.iter().filter_map(|n| parse_hash(n));
        let repos = self.store.repos.iter().filter_map(|r| parse_hash(r.url.strip_prefix("rns://")?.split('/').next()?));
        let git_pages = self.store.git_pages.iter().flat_map(|(git, pages)| [parse_hash(git), parse_hash(pages)]).flatten();
        contacts.chain(propagation).chain(hubs).chain(pages).chain(nodes).chain(repos).chain(git_pages).collect()
    }

    /// Tell the network actor what to keep, if that changed (or `always`:
    /// a new network actor knows nothing yet). Called when the network
    /// starts and about twice a second after, so a contact added or removed,
    /// a hub saved or the propagation node changed is passed on at once and
    /// what is no longer wanted is released.
    pub(super) fn update_retained(&mut self, always: bool) {
        let mut chosen = self.chosen_destinations();
        let mut seen = std::collections::HashSet::new();
        chosen.retain(|hash| seen.insert(*hash));
        chosen.truncate(crate::net::pins::MAX_RETAINED);
        if always || self.retained_sent.as_ref() != Some(&chosen) {
            self.send(NetCommand::Retain(chosen.clone()));
            self.retained_sent = Some(chosen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Settings;
    use crate::store::Store;

    fn told(net: &mut tokio::sync::mpsc::UnboundedReceiver<NetCommand>) -> Option<Vec<Hash>> {
        let commands: Vec<NetCommand> = std::iter::from_fn(|| net.try_recv().ok()).collect();
        commands.into_iter().rev().find_map(|c| match c {
            NetCommand::Retain(list) => Some(list),
            _ => None,
        })
    }

    #[test]
    fn what_the_user_chose_is_retained_and_released_when_it_is_not_chosen() {
        let dir = std::env::temp_dir().join(format!("rettui-retain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        // As at the start: told even though there is nothing yet.
        app.update_retained(true);
        assert_eq!(told(&mut net), Some(Vec::new()));
        app.update_retained(false);
        assert_eq!(told(&mut net), None, "unchanged, so not told again");

        app.set_trust(&"ab".repeat(16), Trust::Untrusted).unwrap();
        app.set_trust(&"cd".repeat(16), Trust::Trusted).unwrap();
        app.set_trust(&"ee".repeat(16), Trust::Blocked).unwrap();
        app.settings.propagation_node = Some("11".repeat(16));
        app.store.saved.push(crate::store::Bookmark { name: "page".into(), url: format!("{}:/page/index.mu", "22".repeat(16)) });
        app.store.identified_nodes.insert("33".repeat(16));
        app.store.repos.push(crate::store::SavedRepo { url: format!("rns://{}/group/repo", "44".repeat(16)), mirror: None });
        app.update_retained(false);
        assert_eq!(
            told(&mut net),
            Some(vec![[0xab; 16], [0xcd; 16], [0x11; 16], [0x22; 16], [0x33; 16], [0x44; 16]]),
            "contacts first; a blocked contact is not kept"
        );

        // A contact removed is left out of the next list, so it is released.
        app.set_trust(&"ab".repeat(16), Trust::Unknown).unwrap();
        app.update_retained(false);
        assert_eq!(told(&mut net).map(|list| list.contains(&[0xab; 16])), Some(false));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_list_is_cut_at_the_limit_with_contacts_kept_first() {
        let dir = std::env::temp_dir().join(format!("rettui-retain-cut-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (mut app, mut net) = crate::app::test_app_with_net(&dir, Settings::default(), Store::default());
        for n in 0..crate::net::pins::MAX_RETAINED as u32 + 50 {
            app.store.identified_nodes.insert(format!("{:08x}{}", n, "00".repeat(12)));
        }
        app.store.contacts.insert("ab".repeat(16), crate::store::Contact { trust: Trust::Trusted, ..Default::default() });
        app.update_retained(true);
        let list = told(&mut net).unwrap();
        assert_eq!(list.len(), crate::net::pins::MAX_RETAINED);
        assert_eq!(list[0], [0xab; 16]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
