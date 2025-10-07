# RotorHazard Lite Documentation

## Hardware Setup

### Required Components
- ESP32-C3 SuperMini development board
- RX5808 FPV receiver module
- Mode selection switch (GND=Node mode, 3.3V=WiFi mode)
- Basic wiring supplies

### Optional Components
- SSD1306 128x64 OLED display (Hertz-hunter compatible)
- I2C pull-up resistors (4.7kΩ) for display

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

### Optional Display Pins (Hertz-hunter Compatible)
```
ESP32-C3 Pin | Display Pin  | Description
-------------|--------------|-------------
GPIO 8       | SDA          | I2C data line
GPIO 9       | SCL          | I2C clock line
3.3V         | VCC          | Power supply
GND          | GND          | Ground
```

### Additional Hertz-hunter Pins (Not Used by RotorHazard Lite)
```
ESP32-C3 Pin | Hertz-hunter | Description
-------------|--------------|-------------
GPIO 21      | PREVIOUS_BUTTON_PIN | Previous button (not used)
GPIO 20      | SELECT_BUTTON_PIN   | Select button (not used)  
GPIO 0       | BATTERY_PIN         | Battery monitoring (not used)
```

These pins are available for future expansion or can be left unconnected.

### Mode Switch (Optional)
```
ESP32-C3 Pin | Switch       | Description
-------------|--------------|-------------
GPIO 10      | Common       | Mode selection
Nothing      | Default      | RotorHazard node mode (floating)
GND          | Position 1   | RotorHazard node mode
3.3V         | Position 2   | WiFi standalone mode
```

**Important**: 
- **Nothing connected** (floating) = RotorHazard node mode (default)
- **GND connected** = RotorHazard node mode  
- **3.3V connected** = WiFi standalone mode


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
