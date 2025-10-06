# RotorHazard Lite Documentation

## Hardware Setup

### Required Components
- ESP32-C3 SuperMini development board (~$1.50 wholesale)
- RX5808 FPV receiver module
- Mode selection switch (GND=Node mode, 3.3V=WiFi mode)
- Basic wiring supplies

### Pin Connections (ESP32-C3 SuperMini - Hertz-hunter Compatible)

```
ESP32-C3 Pin | RX5808 Pin   | Description                    | Hertz-hunter
-------------|--------------|--------------------------------|-------------
GPIO 3       | RSSI         | RSSI analog input (ADC1_CH3)  | RSSI_PIN
GPIO 6       | DATA         | SPI data line (MOSI)          | SPI_DATA_PIN
GPIO 4       | CLK          | SPI clock line                | SPI_CLK_PIN
GPIO 7       | SEL          | SPI chip select (LE pin)      | SPI_LE_PIN
3.3V         | VCC          | Power supply                  | -
GND          | GND          | Ground                        | -
```

**Note**: This pinout is 100% compatible with Hertz-hunter boards! You can use the same PCB/wiring.

### Additional Hertz-hunter Pins (Not Used by RotorHazard Lite)
```
ESP32-C3 Pin | Hertz-hunter | Description
-------------|--------------|-------------
GPIO 21      | PREVIOUS_BUTTON_PIN | Previous button (not used)
GPIO 20      | SELECT_BUTTON_PIN   | Select button (not used)  
GPIO 10      | NEXT_BUTTON_PIN     | Next button (not used)
GPIO 0       | BATTERY_PIN         | Battery monitoring (not used)
```

These pins are available for future expansion or can be left unconnected.

### Mode Switch (Required)
```
ESP32-C3 Pin | Switch       | Description
-------------|--------------|-------------
GPIO 0       | Common       | Mode selection
GND          | Position 1   | RotorHazard node mode
3.3V         | Position 2   | WiFi standalone mode
```

**Important**: The mode switch is required for ESP32-C3 operation. Connect GND for RotorHazard node mode, or 3.3V for WiFi standalone mode.

## PCB Design Considerations

### Layout Guidelines
- Keep RSSI input traces short and shielded
- Use ground plane for noise reduction
- Separate analog and digital sections
- Include proper decoupling capacitors

### Connector Options
- Standard 0.1" headers for ESP32 module
- JST connectors for RX5808 connection
- USB connector for power/programming

## Power Requirements
- Input voltage: 5V USB or 3.3V direct
- Current consumption (ESP32-C3):
  - WiFi mode: ~120mA
  - RotorHazard mode: ~60mA
- Use quality power supply for stable operation
- ESP32-C3 is more power efficient than original ESP32

## Mechanical Considerations
- Enclosure should allow WiFi signal in standalone mode
- Status LED should be visible
- Mode switch should be accessible
- Mounting points for race gate installation
