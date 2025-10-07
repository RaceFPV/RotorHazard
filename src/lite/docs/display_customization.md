# Display Customization Guide

The RotorHazard Lite display system is designed to be easily customizable. All display settings are centralized in `src/display_config.h` for easy modification.

## Quick Customization

### 1. Enable/Disable Display Elements

Edit `src/display_config.h` to turn features on/off:

```cpp
// Show/hide different display elements
#define DISPLAY_SHOW_RSSI_BAR         true   // Visual RSSI bar
#define DISPLAY_SHOW_CROSSING         true   // "CROSS!" indicator
#define DISPLAY_SHOW_FREQUENCY        true   // Current frequency
#define DISPLAY_SHOW_LAP_TIMES        true   // Lap timing info
#define DISPLAY_SHOW_BEST_LAP         true   // Best lap time
```

### 2. Change Update Speed

```cpp
// Update intervals (milliseconds)
#define DISPLAY_UPDATE_INTERVAL_MS    500    // Normal updates
#define DISPLAY_RACE_UPDATE_MS        250    // Faster during race
```

### 3. Customize Text

```cpp
// Text labels
#define DISPLAY_TITLE_READY           "RotorHazard Lite"
#define DISPLAY_TITLE_RACE            "RACE ACTIVE"
#define DISPLAY_STATUS_READY          "Ready"
#define DISPLAY_CROSSING_TEXT         "CROSS!"
```

### 4. Adjust Layout

```cpp
// Y positions (vertical spacing)
#define DISPLAY_Y_TITLE               10
#define DISPLAY_Y_STATUS              25
#define DISPLAY_Y_FREQUENCY           40
#define DISPLAY_Y_RSSI                55
#define DISPLAY_Y_LAP_COUNT           25
#define DISPLAY_Y_CURRENT_LAP         40
#define DISPLAY_Y_BEST_LAP            55

// X positions (horizontal spacing)
#define DISPLAY_X_CROSSING            80
```

## Display Modes

### Ready Mode
Shows when not racing:
- Title: "RotorHazard Lite"
- Status: "Ready"
- Frequency (if enabled)
- RSSI level and threshold
- Crossing indicator (if enabled)

### Race Mode
Shows during active race:
- Title: "RACE ACTIVE"
- Lap count
- Current lap time
- Best lap time (if enabled)

## Advanced Customization

### Adding New Display Elements

1. Add configuration in `display_config.h`:
```cpp
#define DISPLAY_SHOW_NEW_FEATURE      true
#define DISPLAY_Y_NEW_FEATURE         35
```

2. Add drawing function in `display_manager.cpp`:
```cpp
void DisplayManager::drawNewFeature() {
    if (DISPLAY_SHOW_NEW_FEATURE) {
        _display.drawStr(0, DISPLAY_Y_NEW_FEATURE, "New Feature");
    }
}
```

3. Call the function in appropriate display modes.

### Changing Fonts

```cpp
// Available fonts (choose one)
#define DISPLAY_FONT_MAIN             u8g2_font_ncenB08_tr
#define DISPLAY_FONT_SMALL            u8g2_font_ncenB06_tr
#define DISPLAY_FONT_LARGE            u8g2_font_ncenB10_tr
```

### Custom Display Modes

Add new display modes by:
1. Adding a new `showCustomMode()` function
2. Adding configuration options
3. Calling the function from `update()`

## Performance Notes

- Display updates are throttled to prevent timing interference
- Race mode updates faster than ready mode
- All display operations are non-blocking
- No impact on timing accuracy

## Troubleshooting

- **Display not showing**: Check I2C connections (SDA=pin 8, SCL=pin 9)
- **Text overlapping**: Adjust Y position constants
- **Updates too slow/fast**: Modify update interval constants
- **Missing elements**: Check feature enable flags

## Example Customizations

### Minimal Display
```cpp
#define DISPLAY_SHOW_RSSI_BAR         false
#define DISPLAY_SHOW_FREQUENCY        false
#define DISPLAY_SHOW_BEST_LAP         false
```

### Racing Focus
```cpp
#define DISPLAY_TITLE_RACE            "RACING!"
#define DISPLAY_RACE_UPDATE_MS        100    // Very fast updates
```

### Debug Mode
```cpp
#define DISPLAY_TITLE_READY           "DEBUG MODE"
#define DISPLAY_SHOW_CROSSING         true
#define DISPLAY_UPDATE_INTERVAL_MS    100
```
