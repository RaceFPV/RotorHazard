#ifndef DISPLAY_CONFIG_H
#define DISPLAY_CONFIG_H

// Display Configuration - Easy to customize what's shown on screen

// Update intervals (milliseconds)
#define DISPLAY_UPDATE_INTERVAL_MS    500    // How often to update display
#define DISPLAY_RACE_UPDATE_MS        250    // Faster updates during race

// Display layouts and content
#define DISPLAY_SHOW_RSSI_BAR         true   // Show visual RSSI bar
#define DISPLAY_SHOW_CROSSING         true   // Show "CROSS!" indicator
#define DISPLAY_SHOW_FREQUENCY        true   // Show current frequency
#define DISPLAY_SHOW_LAP_TIMES        true   // Show lap timing info
#define DISPLAY_SHOW_BEST_LAP         true   // Show best lap time

// Text and formatting
#define DISPLAY_TITLE_READY           "RotorHazard Lite"
#define DISPLAY_TITLE_RACE            "RACE ACTIVE"
#define DISPLAY_STATUS_READY          "Ready"
#define DISPLAY_STATUS_INIT           "Initializing..."
#define DISPLAY_CROSSING_TEXT         "CROSS!"

// Font settings (using more stable fonts)
#define DISPLAY_FONT_MAIN             u8g2_font_5x7_tr
#define DISPLAY_FONT_SMALL            u8g2_font_5x7_tr

// Layout positions
#define DISPLAY_X_OFFSET              2     // X offset to prevent text cut-off
#define DISPLAY_Y_TITLE               10
#define DISPLAY_Y_STATUS              25
#define DISPLAY_Y_FREQUENCY           40
#define DISPLAY_Y_RSSI                55
#define DISPLAY_Y_LAP_COUNT           25
#define DISPLAY_Y_CURRENT_LAP         40
#define DISPLAY_Y_BEST_LAP            55
#define DISPLAY_Y_CROSSING            25

// Layout positions (X coordinates)
#define DISPLAY_X_CROSSING            80

// RSSI bar settings
#define DISPLAY_RSSI_BAR_Y            60
#define DISPLAY_RSSI_BAR_MAX_WIDTH    100

#endif // DISPLAY_CONFIG_H
