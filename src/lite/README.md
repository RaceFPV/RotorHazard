# RotorHazard Lite - ESP32-C3 Timing System

A dual-mode ESP32-C3-based race timer that works as both:
- **Standalone Mode**: WiFi-enabled web interface for basic timing
- **RotorHazard Node Mode**: Full compatibility with RotorHazard server

## Quick Start

### Standalone Mode (WiFi)
1. Connect GPIO 10 to 3.3V (or leave floating for default)
2. Power on ESP32-C3
3. Connect to WiFi network "rh-lite-XXXX"
4. Open browser to http://192.168.4.1
5. Start timing!

### RotorHazard Node Mode (USB)
1. Leave GPIO 10 floating (default) or connect to GND
2. Connect ESP32-C3 to PC/Pi via USB
3. Add as timing node in RotorHazard server
4. Full RotorHazard features available!

## Hardware Requirements

- ESP32-C3 SuperMini development board 
- RX5808 FPV receiver module
- Mode selection switch (GND=Node, 3.3V=WiFi)

## Pin Configuration (ESP32-C3 SuperMini - Hertz-hunter Compatible)

```cpp
#define RSSI_INPUT_PIN      3     // ADC1_CH3 input from RX5808 (Hertz-hunter RSSI_PIN)
#define RX5808_DATA_PIN     6     // SPI MOSI to RX5808 (Hertz-hunter SPI_DATA_PIN)
#define RX5808_CLK_PIN      4     // SPI SCK to RX5808 (Hertz-hunter SPI_CLK_PIN)
#define RX5808_SEL_PIN      7     // SPI CS to RX5808 (Hertz-hunter SPI_LE_PIN)
#define MODE_SWITCH_PIN     10    // Mode selection switch
#define STATUS_LED_PIN      2     // Status LED (Hertz-hunter BUZZER_PIN)
```

## Building

```bash
cd RotorHazard/src/lite/
pio run -e esp32-c3-supermini --target upload
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
- High-precision timing using FreeRTOS tasks
- All RotorHazard features supported
- WiFi disabled for maximum timing accuracy
- Single-core RISC-V processor

## Development

See `docs/` folder for detailed documentation:
- `hardware.md` - PCB design and wiring
- `building.md` - Compilation and flashing
- `api.md` - Web API reference
