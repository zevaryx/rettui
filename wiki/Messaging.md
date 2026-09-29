The Messages tab: LXMF messages with other Reticulum users (Sideband,
MeshChat, NomadNet and other LXMF clients).

- **Conversations:** a conversation per peer, with unread counts in the
  list and the sidebar. Start one by address (`n`), from the Network tab, or
  from an `lxmf@` link on a page.
- **Drafts:** what you write (and attach) stays with its conversation.
  Opening another one doesn't carry it over, and coming back brings it back.
  A message that fails to send goes back to the conversation it was for.
- **Message states:** your messages show *sending…*, ✓ (delivered), *✓ via
  propagation node*, or *failed* with the reason.
- **Delivery modes:**
  - **Auto** (default): tries direct delivery first. If that fails, it hands
    the message to your propagation node.
  - **Direct:** delivers over a Link and waits for the recipient's proof.
  - **Propagated:** always goes through the propagation node.
- **Stamps and ratchets:** if the recipient's announce asks for a stamp,
  rettui generates it. It also generates the propagation node's stamp, and
  encrypts to the recipient's ratchet when one is known.
- **Propagation nodes:** the ones you hear are listed in the Network tab.
  Pick one with `p`. rettui syncs every `sync_interval_mins` (30 by default)
  or whenever you press `S`. A sync identifies you to the node, downloads your
  messages, then tells the node to delete them.
- **Attachments:**
  - The first image goes in the LXMF image field, which Sideband and MeshChat
    show inline. Other files are sent as file attachments.
  - Received attachments are saved automatically, and images get an inline
    preview. `o` opens the newest attachment with your desktop's default app.
  - Many clients refuse direct transfers over about 1 MB, and rettui warns
    before sending that much.
- **Signatures:** incoming messages are checked against the sender's key. If
  the key isn't known, rettui looks it up first (up to 10 s). Messages that
  still can't be checked are marked *unverified*.
- **Peers who are offline:** public keys from announces are kept, so you can
  write to a peer you heard earlier even while they're away.
- **Announces:** rettui announces your LXMF address when it starts (unless
  you turn that off), every `announce_interval_mins` (360 by default, 0 for
  never), and whenever you press `A`.
