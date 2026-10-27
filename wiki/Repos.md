The Repos tab (`0` in the terminal, **Repos** in the web UI) reads and
writes git repositories hosted on [rngit](https://github.com/zevaryx/rngit-rs)
nodes over Reticulum: their branches and tags, releases and the files
attached to them, and work documents (issues and proposals) with their
comments. It talks to the node the way the `rngit` command does, so
repositories, releases and documents made with one show in the other.

rettui can keep a [copy of a repository on this computer](#a-copy-on-this-computer).
A copy can be [sent to another repository](#sending-a-copy-to-another-repository).
Committing isn't in rettui: use `git` with rngit's `git-remote-rns` helper for that.
rettui can also [host repositories](Hosting-a-Node#rngit-repositories) for others to read.

## Adding a repository

A repository's address is an `rns://` URL: the node's address, a group
and the repository, as `rns://<node>/<group>/<repo>` (the `.git` at the
end is optional).

- `n` in the list (**+ Add** in the web UI) adds one by its URL.
  `rns://` can be left out.
- An `rns://` link in a message, a channel or a NomadNet page opens the
  repository in the Repos tab, as does typing it into the Browser's
  address bar (`g`). In the terminal it then asks whether to copy the
  repository to this computer (type `y`; it asks only if there is no copy yet).
  In the web UI, a link in a message or channel has a **⇩ copy** link beside
  it, which asks before it copies.
- rngit nodes that announce are listed under the repositories you keep, as
  **⌂ node** and its address, nearest first (in the web UI, under
  *rngit nodes heard*). A node can't list what it hosts, so opening one
  (`Enter`, or a click in the web UI) shows the node's pages in the
  [Browser](NomadNet-Browsing). To add a repository on it, press `n` with the
  node picked (**+** beside it in the web UI): the address starts with the node's.
- `D` (**×** in the web UI) removes a repository from the list (on the
  node, nothing changes), and `y` copies its URL.

## Reading a repository

`Enter` opens the picked repository and asks its node for four things,
shown as tabs across the top (`←` `→` or `Tab` switch between them, and
`r` asks again):

- **Branches and tags:** every branch and tag, the commit it points at,
  and the branch `HEAD` is on; above them, whether you have a
  [copy](#a-copy-on-this-computer) and how old it is.
- **Releases:** each release, with its tag, when it was
  made, by whom, and how many files it has. `Enter` opens one with its
  notes and files; `Enter` (or `g`) on a file downloads it. Each file is
  checked against the release's signed manifest before it's kept: a file
  that doesn't match, or a manifest not signed by whoever signed the files,
  is refused and says why. Downloads go in the downloads folder (see
  [Data and Storage](Data-and-Storage)) and never replace a file already
  there. In the web UI, the browser downloads it.
- **Work:** the repository's work documents, in three lists (active,
  proposed and completed). `s` shows the next list. `Enter` opens one with
  its text and comments. A document signed by its author says
  **✓ signed by them**; one whose signature doesn't match its text says
  **✗ signature doesn't check out**.
- **Permissions:** the repository's `.allowed` file, if the node lets you
  read it (it needs `adm`).

A node that can't be reached, that refuses, or doesn't have what was
asked says so where the list would be.

## A copy on this computer

`m` on an open repository (**Copy to this computer** in the web UI) makes
a copy of it here: every branch and tag the node has, with their history.
It is a bare repository, so it has the history and no working files, and
you can clone from it or read it with `git` (`git clone <folder> work`).
Afterwards `m` (**Update the copy**) brings the copy up to date, taking
only what's new, and removes the branches and tags the node no longer has,
so the copy matches the node. A repository with a copy is marked **⇩** in
the list, and the Branches and tags pane says what the copy holds and
when it was updated. In the web UI, `m` does the same.

`M` (**Delete the copy**) deletes the copy from this computer, after you
type `y`; nothing on the node changes. Taking a repository off the list
deletes its copy too.

- The copies are in the `repos/` folder of the [data folder](Data-and-Storage),
  by node, group and name (`repos/<node>/<group>/<name>.git`). rettui picks
  the folder; neither UI can choose another, and the web UI never reads or
  writes files elsewhere.
- A copy changes nothing outside that folder: rettui reads a repository
  from a node as data and runs no program from it. A node that sends a
  branch or tag with a name git wouldn't allow is refused, and nothing of
  that copy is kept.
- A copy may come to 256 MB in one go. A bigger repository isn't copied, and
  says so. How long a copy may take is **rngit timeout** (below). A copy that
  fails to be made leaves nothing behind, and one that fails to be updated
  stays as it was. rettui can't say how far a copy has got, only that it
  is going.
- Two copies of one repository at once aren't made; the second is refused.
- Names Windows can't have as a folder (`con`, `nul`, a name ending in a
  dot) can't be copied. Two repositories whose names differ only in capitals
  can't both be copied, as they would share a folder on some systems.
- Backups don't include `repos/`: copy again from the node after a restore.

### Reading the copy without the node

`Enter` on a branch or tag in the Branches and tags pane (a click in the web UI)
looks inside the copy: the files at the top, then folders (`Enter` opens one,
or a file to read its text), `Esc` or **Back** one step up, and `L` (**Log**)
the newest 200 commits. Nothing is asked of the node, so it works with no
connection. If the node doesn't answer when a repository is opened, its
branches and tags are the copy's, and the pane says so.

- A file is shown as text up to 256 KB, with control characters taken out; a
  file that isn't text (or isn't valid UTF-8) says so and shows nothing.
  Symbolic links show as links; the folder of a submodule isn't in the copy.
- Only the copy's own files are read, and a path that tries to leave the
  repository is refused. The web UI names a repository, a branch or tag and a
  path inside it, and no folder on this computer.
- `rngit log DIR REF`, `rngit tree DIR REF [PATH]` and `rngit show DIR REF PATH`
  of the `rngit` command do the same on a copy's folder.
- The copy is as old as its last update (`m`): it shows what the node had then.
  Releases and work documents aren't in a copy and need the node.

### Sending a copy to another repository

`u` on an open repository that has a copy (**Send to…** in the web UI) sends
the copy to another repository, on this node or another: every branch and
tag in it, with their history. It asks for the repository
(`rns://<node>/<group>/<repo>`), and the repository is made there if the node
hasn't got it (which needs the node's `c` permission). Then it asks what to do
with a branch the other repository has that differs from the copy:

- **add** (the default) sends what is new and leaves such branches as they
  are, and says which they were;
- **overwrite** replaces them with the copy's;
- **same** overwrites them too, and also deletes the branches and tags the
  copy doesn't have, so the other repository ends up the same as the copy.

It needs the other node's write permission for your identity. Only what the
other repository lacks is sent, and a second send of an unchanged copy sends
nothing. Nothing is changed on the repository the copy is of, and a copy
can't be sent to its own repository. The copy being updated is not sent at
the same time. The web UI names the two repositories and no folder.

## Writing

What the node lets you do depends on its permissions for your identity.
A refused change says the node doesn't allow you, and changes nothing.

- **Work documents:** `w` writes a new one (a title, then its text, in
  Markdown), `p` proposes one. On an open document, `c` adds a comment,
  `e` changes its title and text, `x` marks it done (or active again), and
  `X` deletes it, after you type `y`. In the web UI these are buttons, and
  the text is written in a box of its own.
- **Releases:** `P` publishes a release of a tag already pushed to the
  repository: its tag, its notes, then the files to attach (paths on this
  computer, separated by spaces; put a path with spaces in quotes). In the
  web UI, the browser picks the files, never files from the computer
  rettui runs on. Files may come to 32 MB in all. `X` on an open release
  deletes it after you type `y`; its tag stays in the repository.

## Who you are to a node

rettui asks a node as your rettui identity, so the node's permissions for
you are those of your LXMF identity. If you already use the `rngit`
command, with access granted to its identity, turn on **Use rngit's
identity** (Settings, Browsing, `rngit_identity`) and rettui uses that
identity file instead (`~/.rngit/client_identity`, or the folder in
`$RNGIT_CONFIG`). Only the terminal UI can turn it on, since it reads
another program's key.

**rngit timeout** (`rngit_timeout_secs`, 120 seconds) is how long a
request may take before it's given up, from 10 seconds to an hour. A slow
link and a large file may need more.

Answers are capped so a node can't use up rettui's memory: a file of up
to 64 MB, 10,000 branches and tags, and 2,000 entries in a list.
