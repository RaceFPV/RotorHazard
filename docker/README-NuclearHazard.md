# NuclearHazard Docker Setup

## Requirements

- Raspberry Pi 3, 4, or 5
- NuclearHazard timing board
- **External power supply for the NuclearHazard board** (USB power from Pi alone is not sufficient)
- Docker installed on the Pi

## Quick Start

**Run this command on your Raspberry Pi:**

```bash
./docker/run-prod-nuclear-pi.sh
```

## What Happens

1. Pi configures itself for NuclearHazard hardware
2. Docker image downloads
3. Pi reboots (~1 minute)
4. NuclearHazard starts automatically

## Access

After reboot, open in your browser:

```
http://localhost
```

Or from another device on your network:

```
http://<pi-ip-address>
```

**Login:** `NuclearHazard` / `nuclearhazard`

## That's It!

NuclearHazard will auto-start every time your Pi boots. No additional setup needed.

---

## Troubleshooting

### Check if running
```bash
docker ps
```

### View logs
```bash
docker logs -f nuclearhazard-server
```

### Restart
```bash
docker restart nuclearhazard-server
```

### Stop
```bash
docker stop nuclearhazard-server
```

### No nodes detected / Can't communicate with processor
- **Check power**: NuclearHazard board requires external power supply (not just USB from Pi)
- Check that the board is properly seated on the GPIO header
- Check logs for serial port errors

### Firmware flash fails
If automatic GPIO reset doesn't work:
1. Stop the container: `docker stop nuclearhazard-server`
2. Power off Pi
3. Install Boot0 jumper on NuclearHazard board
4. Power on Pi
5. Start container: `docker start nuclearhazard-server`
6. Flash firmware via web UI
7. Power off, remove Boot0 jumper, power on

### GPIO errors on Pi 5
The container automatically creates a `/dev/gpiochip4` symlink for Pi 5 compatibility. If you see GPIO errors, try:
```bash
docker restart nuclearhazard-server
```

### Reset to fresh state
```bash
docker rm -f nuclearhazard-server
sudo rm /etc/nuclearhazard-setup-complete
rm -rf ./rh-data
./docker/run-prod-nuclear-pi.sh
```

---

## Optional: WiFi Hotspot

Set up automatic WiFi hotspot fallback (useful for field use):

```bash
./docker/setup-wifi-hotspot.sh
```

Then edit your WiFi credentials:
```bash
nano ~/wifi_config.txt
```
- Line 1: Your WiFi SSID
- Line 2: Your WiFi password

On boot, the Pi will:
1. Try to connect to your configured WiFi
2. If that fails, create a hotspot:
   - SSID: `NuclearHazard`
   - Password: `nuclearhazard`

To disable: `sudo systemctl disable nuclearhazard-hotspot.service`
