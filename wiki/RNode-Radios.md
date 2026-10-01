An RNode is a LoRa radio that Reticulum uses as an interface: with one,
rettui reaches others nearby (up to kilometres away, depending on the
terrain and antennas) with no internet at all. Many ready-made LoRa boards (from LilyGO, Heltec, RAK and
others) become an RNode once the RNode firmware is flashed onto them.

rettui doesn't flash radios itself. The tools below are made alongside the
firmware and know each board; this page only says which to use and how
they fit with rettui. Where the projects' own instructions differ from this
page, theirs are right. The RNode firmware's README warns about fake,
machine-written RNode guides and tools; it lists the trustworthy ones, and
the [RNode Manual by RNS Moscow](https://docs.rns.moscow/rnode-manual/) it
recommends covers far more than this page.

## Which firmware

- **[RNode Firmware](https://github.com/markqvist/RNode_Firmware)**, the
  official one: the board is a radio for the Reticulum running on the
  computer or phone it's connected to. Start with this one.
- **[microReticulum_Firmware](https://github.com/attermann/microReticulum_Firmware)**:
  a fork of the RNode firmware with a Reticulum stack
  ([microReticulum](https://github.com/attermann/microReticulum)) built in.
  By default it works like any RNode; switched to its *Transport Mode*, the
  board runs on its own as a transport node, passing traffic on with no
  computer attached (a solar-powered repeater on a roof, say). Its README
  says how to turn Transport Mode on, and not to connect another Reticulum
  (rettui included) to a board in that mode.

## Flashing with rnodeconf

`rnodeconf` comes with Python Reticulum (the `rns` package). Plug the
board in over USB, then:

```
pip install rns --upgrade
rnodeconf --autoinstall
```

It asks which serial port, board and frequency band, downloads the latest
firmware, flashes it and sets the board up. Later, `rnodeconf --update
<port>` updates the firmware and `rnodeconf --info <port>` shows what's on
the board. On Linux, if the port can't be opened, add yourself to the group
that owns it (often `dialout`) and log in again.

For **microReticulum_Firmware**, point rnodeconf at that project's
releases (the trailing slash matters):

```
rnodeconf --clear-cache
rnodeconf --autoinstall --fw-url https://github.com/attermann/microReticulum_Firmware/releases/
```

rnodeconf keeps the firmware it downloads and uses it again, and the two
projects can publish the same version numbers and file names. Clear its
cache (`rnodeconf --clear-cache`, which removes only downloaded firmware)
whenever you switch between them, or it may flash the one it downloaded
before. Going back to the official firmware is `rnodeconf --clear-cache`
then `rnodeconf --autoinstall`.

## Flashing in a browser

[Liam Cottle's RNode Flasher](https://liamcottle.github.io/rnode-flasher/),
which the RNode firmware's README suggests for those who'd rather not use
a command line, flashes most supported boards from a web page. It needs a
browser with Web Serial: Chrome, Edge or another Chromium-based browser on
a computer (not Firefox or Safari).

1. Download the firmware `.zip` for your board from the releases of the
   firmware you chose:
   [RNode Firmware](https://github.com/markqvist/RNode_Firmware/releases)
   or [microReticulum_Firmware](https://github.com/attermann/microReticulum_Firmware/releases)
   (the flasher calls it "Transport Node Firmware" and links both).
2. Open the flasher, pick your board, put it in DFU mode if the page says
   to (nRF52 boards such as the RAK4631), select the `.zip` and flash it.
3. On a board that has never had RNode firmware, choose **Provision**.
4. After every flash, choose **Set Firmware Hash**; without it the board
   reports its firmware as corrupt.

## Using it in rettui

Add the radio as an interface in the **Reticulum** tab (in either UI): add
an interface (`a` in the terminal) of type **RNode (LoRa)** and set:

- **Port:** the serial device, such as `/dev/ttyUSB0` or `/dev/ttyACM0` on
  Linux, `/dev/cu.usbserial-…` on macOS or `COM3` on Windows.
- **Frequency, Bandwidth, Spreading factor, Coding rate:** radios only hear
  each other when all four match, so use what people near you use. The
  new interface starts from an example; change it to your area's.
- **TX power:** in dBm. Stay within the frequency band and power your
  country allows, and within what the board can do.

Then restart Reticulum (`Ctrl-R`, or **Restart Reticulum** in the web UI)
to bring it up; the Status tab's Interfaces list shows whether it's
online, and the log says why if it isn't. If another
program such as rnsd runs the shared instance, add the radio to that
program's config instead. With Docker, the container needs the device:
add it under the service in the Compose file, for example
`devices: ["/dev/ttyUSB0:/dev/ttyUSB0"]` (see [Docker](Docker)).

More on the interface's options (airtime limits, flow control, beacons) is
in [Settings and Reticulum Config](Settings-and-Reticulum-Config) and
Reticulum's manual
([RNode LoRa interface](https://reticulum.network/manual/interfaces.html#rnode-lora-interface)).
