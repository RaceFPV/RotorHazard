#ifndef CONFIG_H
#define CONFIG_H

// Hardware pin definitions
#define RSSI_INPUT_PIN      36    // ADC1_0 - RSSI input from RX5808
#define RX5808_DATA_PIN     23    // SPI MOSI to RX5808 module
#define RX5808_CLK_PIN      18    // SPI SCK to RX5808 module  
#define RX5808_SEL_PIN      5     // SPI CS to RX5808 module
#define MODE_SWITCH_PIN     0     // Mode selection switch (boot button)
#define STATUS_LED_PIN      2     // Built-in LED for status

// UART pins for RotorHazard communication (built-in)
#define UART_TX_PIN         1     // TX to Pi/PC
#define UART_RX_PIN         3     // RX from Pi/PC
#define UART_BAUD_RATE      115200

// Mode selection
#define WIFI_MODE           HIGH  // Switch up = WiFi/Standalone mode
#define ROTORHAZARD_MODE    LOW   // Switch down = RotorHazard node mode

// RX5808 frequency constants  
#define MIN_FREQ            5645  // Minimum frequency (MHz)
#define MAX_FREQ            5945  // Maximum frequency (MHz)
#define DEFAULT_FREQ        5800  // Default frequency (MHz)

// Timing configuration
#define TIMING_INTERVAL_MS  1     // Core timing loop interval
#define RSSI_SAMPLES        10    // Number of RSSI samples to average
#define CROSSING_THRESHOLD  50    // Default RSSI threshold for crossing detection

// WiFi configuration
#define WIFI_AP_SSID_PREFIX "RaceTimer"
#define WIFI_AP_PASSWORD    ""    // Open network for simplicity
#define WEB_SERVER_PORT     80

// Data storage
#define MAX_LAPS_STORED     100   // Maximum laps to store in memory
#define MAX_PILOTS          2     // Maximum pilots in standalone mode

// Debug settings
#define DEBUG_SERIAL        1     // Enable debug output
#define DEBUG_TIMING        0     // Enable timing debug output

#if DEBUG_SERIAL
  #define DEBUG_PRINT(x)    Serial.print(x)
  #define DEBUG_PRINTLN(x)  Serial.println(x)
#else
  #define DEBUG_PRINT(x)
  #define DEBUG_PRINTLN(x)
#endif

#if DEBUG_TIMING
  #define TIMING_PRINT(x)   Serial.print(x)
  #define TIMING_PRINTLN(x) Serial.println(x)
#else
  #define TIMING_PRINT(x)
  #define TIMING_PRINTLN(x)  
#endif

#endif // CONFIG_H
