#!/usr/bin/env bash
# Build and push all RotorHazard Docker images to Docker Hub.
# Usage: ./build-and-push.sh [OPTIONS]
#
# Options:
#   --no-push     Build only, don't push to Docker Hub
#   --only=NAME   Build only one image (rotorhazard, pi, nuclearpi)
#
# Examples:
#   ./build-and-push.sh                    # Build and push all
#   ./build-and-push.sh --no-push          # Build all without pushing
#   ./build-and-push.sh --only=nuclearpi   # Build and push only NuclearHazard

set -e

# Determine repo root (script can be run from docker/ or repo root)
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ "$(basename "$SCRIPT_DIR")" == "docker" ]]; then
    REPO_ROOT="$(dirname "$SCRIPT_DIR")"
else
    REPO_ROOT="$SCRIPT_DIR"
fi

cd "$REPO_ROOT"
echo "Building from: $REPO_ROOT"

# Parse arguments
PUSH=true
ONLY=""

for arg in "$@"; do
    case $arg in
        --no-push)
            PUSH=false
            ;;
        --only=*)
            ONLY="${arg#*=}"
            ;;
    esac
done

# Set up buildx builder if not exists
if ! docker buildx inspect multi >/dev/null 2>&1; then
    echo "Creating buildx builder 'multi'..."
    docker buildx create --use --name multi
else
    docker buildx use multi
fi

# Build flags
if [ "$PUSH" = true ]; then
    PUSH_FLAG="--push"
    echo "Mode: Build and push"
else
    PUSH_FLAG="--load"
    echo "Mode: Build only (no push)"
fi

echo ""

# Image definitions
# Format: name:dockerfile:platforms:tag
IMAGES=(
    "rotorhazard:Dockerfile:linux/amd64:racefpv/rotorhazard:latest"
    "pi:Dockerfile.pi:linux/arm64:racefpv/rotorhazard-pi:latest"
    "nuclearpi:Dockerfile.nuclearpi:linux/arm64:racefpv/rotorhazard-nuclearpi:latest"
)

build_image() {
    local name="$1"
    local dockerfile="$2"
    local platforms="$3"
    local tag="$4"

    echo "=============================================="
    echo "Building: $name"
    echo "  Dockerfile: docker/$dockerfile"
    echo "  Platform:   $platforms"
    echo "  Tag:        $tag"
    echo "=============================================="

    # For single platform with --load, we can't use --push
    # For multi-platform or --push, use the appropriate flag
    if [ "$PUSH" = true ]; then
        docker buildx build \
            --platform "$platforms" \
            -f "docker/$dockerfile" \
            -t "$tag" \
            --push \
            .
    else
        # --load only works with single platform
        docker buildx build \
            --platform "$platforms" \
            -f "docker/$dockerfile" \
            -t "$tag" \
            --load \
            .
    fi

    echo ""
    echo "✓ $name complete"
    echo ""
}

# Build images
for img in "${IMAGES[@]}"; do
    IFS=':' read -r name dockerfile platforms tag <<< "$img"
    
    # Skip if --only specified and doesn't match
    if [ -n "$ONLY" ] && [ "$ONLY" != "$name" ]; then
        continue
    fi

    build_image "$name" "$dockerfile" "$platforms" "$tag"
done

echo "=============================================="
echo "All builds complete!"
echo ""
echo "Images:"
echo "  racefpv/rotorhazard:latest         (amd64)"
echo "  racefpv/rotorhazard-pi:latest      (arm64)"
echo "  racefpv/rotorhazard-nuclearpi:latest (arm64)"
if [ "$PUSH" = true ]; then
    echo ""
    echo "All images pushed to Docker Hub."
fi
echo "=============================================="
