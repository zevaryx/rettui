The web UI can run in a container. Each release publishes an image for
x86-64 and ARM64 to `ghcr.io/rettui-dev/rettui`, tagged with the version (`1.2.0`,
`1.2`, `1`) and `latest`. To use it, remove `build: .` from
[compose.yaml](https://github.com/rettui-dev/rettui/blob/main/compose.yaml) and set `image: ghcr.io/rettui-dev/rettui:latest`,
then `docker compose up -d`. Every other branch has an image too, named
after the branch (`ghcr.io/rettui-dev/rettui:dev` for `dev`); see
[Releases and CI](Development#releases-and-ci).

To build the image yourself instead, from a checkout with its submodules:

```sh
git submodule update --init
docker compose up -d --build
docker compose logs rettui   # the login link (http://127.0.0.1:8740/?token=…)
```

- **Data:** [compose.yaml](https://github.com/rettui-dev/rettui/blob/main/compose.yaml) keeps everything in `./data`
  next to it: the identity, settings, messages, cache, and the Reticulum
  config in `./data/reticulum/config`. rettui writes that config the first
  time it starts.
- **Owner of the files:** `PUID` and `PGID` (default 1000) set the user and
  group rettui runs as, so the files in `./data` belong to you. Use your
  `id -u` and `id -g`. `GUID` also works in place of `PGID`.
- **Network:** Reticulum's default AutoInterface finds peers on the local
  network by multicast, which doesn't leave Docker's default network. Either
  add an interface to `./data/reticulum/config` (for example a
  `TCPClientInterface` to a transport node, which the Reticulum section of the
  web UI can do) and restart Reticulum, or use `network_mode: host` (commented out in
  the compose file) instead of `ports:`.
- **Access:** port 8740 is published on `127.0.0.1` only, so only this
  machine can connect. To reach it from other devices, publish it as
  `"8740:8740"` in `compose.yaml` and add `command: ["--https"]` to the
  service. The web UI is plain HTTP otherwise, and the link and the login
  cross the network unencrypted, where anyone on it can read them (rettui
  says so in its log). **The image accepts plain HTTP by itself**:
  its entrypoint always starts rettui with `--insecure-http`, because rettui
  refuses plain HTTP on the container's own address (`0.0.0.0`) otherwise.
  That is only safe because `compose.yaml` publishes the port on
  `127.0.0.1`. If you publish it any wider (`docker run -p 8740:8740`, a
  Compose file of your own, or `network_mode: host`), add `--https`. The login
  link is still required either way.
  `PORT` changes the port inside the container.
  With `network_mode: host` the web UI is on every interface of the host, so
  use `--https` with it. Other containers on the same Docker network can
  reach the port too, in clear text. The container's log says "Plain HTTP on
  0.0.0.0:…, as --insecure-http allows" even when the port is published on
  `127.0.0.1` only; it is about the address rettui listens on inside the
  container.
- **Transport node:** port 4242 (Reticulum's usual port) is exposed and
  published too, for others to connect to rettui as an entry point.
  Nothing listens on it until the Reticulum config has, under
  `[reticulum]`, `enable_transport = Yes`, and an interface listening on
  `0.0.0.0` port 4242:

  ```
  [[Public entry point]]
    type = TCPServerInterface
    enabled = Yes
    listen_ip = 0.0.0.0
    listen_port = 4242
  ```

  The Reticulum section of the web UI can add both with its **Entry
  point** button; restart Reticulum after. To announce it for interface discovery, add
  `discoverable = Yes` and `reachable_on` (the host's public address or
  name). Remove the `"4242:4242"` line from `ports:` if you don't run one.
- **A node with no web UI:** add `command: node` to the service in the
  compose file (and remove the `"8740:8740"` line from `ports:`), and the
  container runs [`rettui node`](Hosting-a-Node#a-public-node-with-no-ui)
  instead: a transport node on port 4242 and a propagation node, set up the
  first time it starts. Its flags go after it, for example
  `command: ["node", "--reachable-on", "rns.example.org"]`. `docker compose
  logs rettui` shows its status.
- **The node's flags as variables (only with `command: node`):** every flag
  of `rettui node` but `--stats-listen` can be set as an environment
  variable instead, in `environment:` or an `env_file`: `RETTUI_NODE_ENTRY_POINT`,
  `RETTUI_NODE_LISTEN_IP`, `RETTUI_NODE_REACHABLE_ON`, `RETTUI_NODE_NAME`,
  `RETTUI_NODE_NO_ENTRY_POINT` and `RETTUI_NODE_NO_PROPAGATION` (the last two
  take `yes`, `true` or `1` to turn them on, and `no`, `false` or `0` for
  off, in any case). A flag after `node` wins over its variable, and a
  variable that is empty (`RETTUI_NODE_NAME: ""`) counts as not set. The
  container refuses to start, naming the variable, on a value the flag
  wouldn't take, or on two that can't go together (`NO_ENTRY_POINT` with
  `ENTRY_POINT`, `LISTEN_IP` or `REACHABLE_ON`; `NO_PROPAGATION` with `NAME`).
  They are kept like the flags, so the next start needs none of them, and,
  like the flags, `RETTUI_NODE_NO_PROPAGATION` doesn't turn off a propagation
  node that an earlier start turned on. The web UI ignores them. See
  [Hosting a Node](Hosting-a-Node#a-public-node-with-no-ui) for what each
  does.
- **Stats page:** set `STATS_LISTEN` (for example `0.0.0.0:8741`) to also serve
  a read-only JSON summary of the node, totals only, at `/stats.json` on that
  port; it is off otherwise. It has no login, so publish it only to a reverse
  proxy (`"127.0.0.1:8741:8741"` under `ports:`) and not to the internet. It
  works the same with `command: node`. With the propagation node on, it also
  gives its stored messages, limit and peer counts, and the size of the data
  directory and of the volume's file system. See
  [Running](Running#the-stats-endpoint) for the JSON and an nginx example.
  To let a website that has a secret also read every peer heard, set
  `STATS_TOKEN_FILE` to the path of a file in the container that holds it
  (a Docker secret, such as `/run/secrets/stats_token`, is the better way),
  or set `RETTUI_STATS_TOKEN` to the secret in `environment:` (read by
  rettui itself, but `docker inspect` and `/proc` show the environment, and
  programs rettui starts inherit it). Not both. The token is 16 to 256 printable characters with no
  spaces, and the container stops, with the reason, if it isn't. It needs
  `STATS_LISTEN`. See [Hosting a Node](Hosting-a-Node#the-peer-list-for-whoever-has-the-token).
- `docker compose down` stops rettui cleanly; it saves on SIGTERM.
