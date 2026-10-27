The Repos tab (`0` in the terminal, **Repos** in the web UI) reads and
writes git repositories hosted on [rngit](https://github.com/zevaryx/rngit-rs)
nodes over Reticulum: their branches and tags, releases and the files
attached to them, and work documents (issues and proposals) with their
comments. It talks to the node the way the `rngit` command does, so
repositories, releases and documents made with one show in the other.

Cloning and pushing aren't in rettui yet: use `git` with rngit's
`git-remote-rns` helper for that.

## Adding a repository

A repository's address is an `rns://` URL: the node's address, a group
and the repository, as `rns://<node>/<group>/<repo>` (the `.git` at the
end is optional).

- `n` in the list (**+ Add** in the web UI) adds one by its URL.
  `rns://` can be left out.
- An `rns://` link in a message, a channel or a NomadNet page opens the
  repository in the Repos tab, as does typing it into the Browser's
  address bar (`g`).
- rngit nodes that announce are listed under the repositories you keep, as
  **⌂ node** and its address, nearest first (in the web UI, under
  *rngit nodes heard*). A node can't list what it hosts, so opening one
  asks for the group and repository there.
- `D` (**×** in the web UI) removes a repository from the list (on the
  node, nothing changes), and `y` copies its URL.

## Reading a repository

`Enter` opens the picked repository and asks its node for four things,
shown as tabs across the top (`←` `→` or `Tab` switch between them, and
`r` asks again):

- **Branches and tags:** every branch and tag, the commit it points at,
  and the branch `HEAD` is on.
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
