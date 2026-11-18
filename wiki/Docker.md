The web UI can run in a container. Each release publishes an image for
x86-64 and ARM64 to `ghcr.io/zevaryx/rettui`, tagged with the version (`1.2.0`,
`1.2`, `1`) and `latest`. To use it, remove `build: .` from
[compose.yaml](https://github.com/zevaryx/rettui/blob/main/compose.yaml) and set `image: ghcr.io/zevaryx/rettui:latest`,
then `docker compose up -d`. Every other branch has an image too, named
after the branch (`ghcr.io/zevaryx/rettui:dev` for `dev`); see
[Releases and CI](Development#releases-and-ci).

To build the image yourself instead, from a checkout with its submodules:

```sh
git submodule update --init
docker compose up -d --build
docker compose logs rettui   # the login link (http://127.0.0.1:8740/?token=…)
```

- **Data:** [compose.yaml](https://github.com/zevaryx/rettui/blob/main/compose.yaml) keeps everything in `./data`
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
- `docker compose down` stops rettui cleanly; it saves on SIGTERM.
