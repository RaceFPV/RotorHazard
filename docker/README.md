# RotorHazard Docker

## Local development (build from source)

Default compose file: builds from source so devs don’t pull from Docker Hub.

From **repo root**:

```bash
docker-compose -f docker/docker-compose.yml up -d
```

Or from `docker/`:

```bash
docker-compose up -d
```

Dev uses a **named volume** (`rotorhazard-dev-data`) so the repo has no data directory and personal data isn’t committed. To wipe dev data: `docker volume rm rotorhazard-dev-data` (after stopping the container).

## Production / published image (Docker Hub)

Use the pre-built image (e.g. on a Pi or when you don’t have the repo):

```bash
docker pull racefpv/rotorhazard:latest
```

Run with the prod compose file (same ports/volumes, no build):

From **repo root**:

```bash
docker-compose -f docker/docker-compose.prod.yml up -d
```

Or from `docker/`:

```bash
docker-compose -f docker-compose.prod.yml up -d
```

**Without compose** (same behavior, one container):

```bash
# Script (default data dir: ./data)
chmod +x docker/run-prod.sh
./docker/run-prod.sh

# Or custom data dir
./docker/run-prod.sh /opt/rotorhazard/data
```

One-liner (set your own path for the volume):

```bash
docker run -d --name rotorhazard-server --restart unless-stopped -p 5000:5000 -v ./data:/app/data -e RH_DATA_DIR=/app/data --privileged racefpv/rotorhazard:latest
```

| File                      | Use case        | Image source   |
|---------------------------|-----------------|----------------|
| `docker-compose.yml`      | Local dev       | Build from repo |
| `docker-compose.prod.yml` | Production / Pi | Docker Hub     |
| `docker-compose.nuclearpi.yml` | NuclearHazard dev | Build from repo |

## Dockerfiles

| File           | Use case   | Requirements        |
|----------------|------------|---------------------|
| `Dockerfile`   | Non-Pi (x64/amd64) | `reqsNonPi.txt`  |
| `Dockerfile.pi` | Raspberry Pi (arm64) | `requirements.txt` (RPi.GPIO, rpi-ws281x, etc.) |
| `Dockerfile.nuclearpi` | NuclearHazard boards (arm64 only) | `requirements.txt` + rpi5-ws2812, esptool, pillow |

Build from **repo root**:
- Non-Pi: `docker build -f docker/Dockerfile -t rotorhazard .`
- Pi: `docker build -f docker/Dockerfile.pi -t rotorhazard:pi .`
- NuclearHazard: `docker buildx build --platform linux/arm64 -f docker/Dockerfile.nuclearpi -t rotorhazard:nuclearpi .`

## Build and push all images

Use the build script to build and push all images at once:

```bash
# Build and push all images to Docker Hub
./docker/build-and-push.sh

# Build only (no push)
./docker/build-and-push.sh --no-push

# Build only one image
./docker/build-and-push.sh --only=nuclearpi
./docker/build-and-push.sh --only=pi
./docker/build-and-push.sh --only=rotorhazard
```

This builds:
| Image | Platform | Dockerfile |
|-------|----------|------------|
| `racefpv/rotorhazard:latest` | amd64 | `Dockerfile` |
| `racefpv/rotorhazard-pi:latest` | arm64 | `Dockerfile.pi` |
| `racefpv/rotorhazard-nuclearpi:latest` | arm64 | `Dockerfile.nuclearpi` |

## Manual multi-platform build

To build individual images manually:

```bash
docker buildx create --use --name multi  # one-time

# Non-Pi image for amd64
docker buildx build --platform linux/amd64 -f docker/Dockerfile -t racefpv/rotorhazard:latest --push .

# Pi image for arm64
docker buildx build --platform linux/arm64 -f docker/Dockerfile.pi -t racefpv/rotorhazard-pi:latest --push .

# NuclearHazard image for arm64
docker buildx build --platform linux/arm64 -f docker/Dockerfile.nuclearpi -t racefpv/rotorhazard-nuclearpi:latest --push .
```

## NuclearHazard (arm64 only)

NuclearHazard boards require additional Pi configuration and packages. Uses a separate image tag.

**Important:** NuclearHazard board requires external power supply (USB from Pi alone is not sufficient).

See [README-NuclearHazard.md](README-NuclearHazard.md) for detailed setup and troubleshooting.

**Quick start (recommended):**

```bash
chmod +x docker/run-prod-nuclear-pi.sh
./docker/run-prod-nuclear-pi.sh
```

The script automatically configures the Pi, pulls the image, and reboots. After reboot, NuclearHazard is running at `http://localhost`.

**Using docker-compose (dev/build from source):**

```bash
docker-compose -f docker/docker-compose.nuclearpi.yml up -d
```

**Default credentials:** `NuclearHazard` / `nuclearhazard`

**Build and push:**

```bash
docker buildx build --platform linux/arm64 -f docker/Dockerfile.nuclearpi -t racefpv/rotorhazard-nuclearpi:latest --push .
```
