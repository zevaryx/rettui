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
- **Access:** port 8740 is published on every interface of the host, and the
  login link is still required. Change it to `"127.0.0.1:8740:8740"` to allow
  only this machine. `PORT` changes the port inside the container.
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

  The Reticulum section of the web UI can add it; restart Reticulum
  after. To announce it for interface discovery, add
  `discoverable = Yes` and `reachable_on` (the host's public address or
  name). Remove the `"4242:4242"` line from `ports:` if you don't run one.
- `docker compose down` stops rettui cleanly; it saves on SIGTERM.
