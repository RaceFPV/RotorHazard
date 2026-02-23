# NuclearHazard Docker Setup

## Quick Start

**Run this command on your Raspberry Pi:**

```bash
curl -sSL https://raw.githubusercontent.com/RaceFPV/RotorHazard/docker/docker/run-prod-nuclear-pi.sh | bash
```

Or if you have the repo cloned:

```bash
./docker/run-prod-nuclear-pi.sh
```

## What Happens

1. Pi configures itself for NuclearHazard hardware
2. Docker image downloads
3. Pi reboots (takes ~1 minute)
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

**Check if running:**
```bash
docker ps
```

**View logs:**
```bash
docker logs -f nuclearhazard-server
```

**Restart:**
```bash
docker restart nuclearhazard-server
```

**Stop:**
```bash
docker stop nuclearhazard-server
```
