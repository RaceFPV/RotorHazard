# RotorHazard Lite Documentation

## Hardware Setup

### Required Components
- ESP32 development board (NodeMCU-32S recommended)
- RX5808 FPV receiver module
- Mode selection switch (optional - can use boot button)
- Basic wiring supplies

### Pin Connections

```
ESP32 Pin    | RX5808 Pin   | Description
-------------|--------------|-------------
GPIO 36      | RSSI         | RSSI analog input
GPIO 23      | DATA         | SPI data line
GPIO 18      | CLK          | SPI clock line  
GPIO 5       | SEL          | SPI chip select
3.3V         | VCC          | Power supply
GND          | GND          | Ground
```

### Mode Switch (Optional)
```
ESP32 Pin    | Switch       | Description
-------------|--------------|-------------
GPIO 0       | Common       | Mode selection
GND          | Position 1   | RotorHazard mode
Floating     | Position 2   | WiFi standalone mode
```

If no external switch is used, the built-in boot button serves as the mode selector.

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
- Current consumption:
  - WiFi mode: ~150mA
  - RotorHazard mode: ~80mA
- Use quality power supply for stable operation

## Mechanical Considerations
- Enclosure should allow WiFi signal in standalone mode
- Status LED should be visible
- Mode switch should be accessible
- Mounting points for race gate installation
