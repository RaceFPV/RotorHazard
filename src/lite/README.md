# RotorHazard Lite - ESP32 Timing System

A dual-mode ESP32-based race timer that works as both:
- **Standalone Mode**: WiFi-enabled web interface for basic timing
- **RotorHazard Node Mode**: Full compatibility with RotorHazard server

## Quick Start

### Standalone Mode (WiFi)
1. Set mode switch to UP position
2. Power on ESP32
3. Connect to WiFi network "RaceTimer-XXXX"
4. Open browser to http://192.168.4.1
5. Start timing!

### RotorHazard Node Mode (USB)
1. Set mode switch to DOWN position  
2. Connect ESP32 to PC/Pi via USB
3. Add as timing node in RotorHazard server
4. Full RotorHazard features available!

## Hardware Requirements

- ESP32 development board (NodeMCU-32S recommended)
- RX5808 FPV receiver module
- Mode selection switch (or use built-in boot button)

## Pin Configuration

```cpp
#define RSSI_INPUT_PIN      36    // ADC input from RX5808
#define RX5808_DATA_PIN     23    // SPI MOSI to RX5808
#define RX5808_CLK_PIN      18    // SPI SCK to RX5808  
#define RX5808_SEL_PIN      5     // SPI CS to RX5808
#define MODE_SWITCH_PIN     0     // Mode selection switch
#define STATUS_LED_PIN      2     // Status LED
```

## Building

```bash
cd RotorHazard/src/lite/
pio run --target upload
```

## Features

### Standalone Mode
- Web-based timing interface
- Real-time lap display
- Data logging to flash memory
- Mobile-friendly responsive design
- 1-2 pilot support

### RotorHazard Node Mode  
- Full protocol compatibility
- High-precision timing
- All RotorHazard features supported
- WiFi disabled for maximum timing accuracy

## Development

See `docs/` folder for detailed documentation:
- `hardware.md` - PCB design and wiring
- `building.md` - Compilation and flashing
- `api.md` - Web API reference
