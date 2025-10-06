#include <Arduino.h>
#include "config.h"
#include "timing_core.h"
#include "standalone_mode.h"
#include "node_mode.h"

// Global objects
TimingCore timing;
StandaloneMode standalone;
NodeMode node;

// Current operation mode
enum OperationMode {
  MODE_STANDALONE,
  MODE_ROTORHAZARD
};

OperationMode current_mode;
bool mode_initialized = false;

// Function declarations
void checkModeSwitch();
void initializeMode();
void serialEvent();

void setup() {
  Serial.begin(UART_BAUD_RATE);
  
  // Wait for serial connection (like Arduino)
  while (!Serial) {
    delay(10);
  }
  
  // Additional delay for ESP32-C3 USB CDC
  #ifdef ARDUINO_USB_CDC_ON_BOOT
  delay(1000); // Give time for USB serial to stabilize
  #endif
  
  // Initialize mode selection pin (floating=Node, GND=WiFi, HIGH=Node)
  pinMode(MODE_SWITCH_PIN, INPUT_PULLUP);
  pinMode(STATUS_LED_PIN, OUTPUT);
  
  // Brief startup flash
  digitalWrite(STATUS_LED_PIN, HIGH);
  delay(500);
  digitalWrite(STATUS_LED_PIN, LOW);
  delay(200);
  
  // Initialize core timing system (always active)
  timing.begin();
  
  // Determine initial mode
  bool initial_switch_state = digitalRead(MODE_SWITCH_PIN);
  
  // Force initial mode detection (bypass the 100ms delay)
  current_mode = (initial_switch_state == LOW) ? MODE_STANDALONE : MODE_ROTORHAZARD;
  
  initializeMode();
}

void loop() {
  // Check for mode changes
  checkModeSwitch();
  
  // Always process core timing (now handled by FreeRTOS task)
  timing.process();
  
  // Process mode-specific functions
  if (current_mode == MODE_STANDALONE) {
    standalone.process();
  } else {
    node.process();
  }
  
  // Handle serial communication (like Arduino)
  serialEvent();
  
  // Brief yield to prevent watchdog issues (ESP32-C3 single core)
  vTaskDelay(pdMS_TO_TICKS(10));
}

// Serial event handler (like Arduino)
void serialEvent() {
  // Only handle serial in RotorHazard node mode
  if (current_mode == MODE_ROTORHAZARD) {
    node.handleSerialInput();
  }
}

void checkModeSwitch() {
  static unsigned long last_check = 0;
  static bool last_switch_state = -1; // Initialize to invalid state to force first check
  
  // Only check every 100ms to avoid bouncing
  if (millis() - last_check < 100) {
    return;
  }
  
  bool current_switch_state = digitalRead(MODE_SWITCH_PIN);
  
  if (current_switch_state != last_switch_state) {
    // Determine new mode
    // LOW (GND) = WiFi mode, HIGH (floating/pullup) = RotorHazard mode (default)
    OperationMode new_mode;
    if (current_switch_state == LOW) {
      new_mode = MODE_STANDALONE;  // Switch to GND = WiFi mode
    } else {
      new_mode = MODE_ROTORHAZARD; // Switch to 3.3V or floating = RotorHazard mode (default)
    }
    
    if (new_mode != current_mode || !mode_initialized) {
      current_mode = new_mode;
      initializeMode();
    }
    
    last_switch_state = current_switch_state;
  }
  
  last_check = millis();
}

void initializeMode() {
  if (current_mode == MODE_STANDALONE) {
    DEBUG_PRINTLN("Initializing mode: STANDALONE/WIFI");
    
    // Shutdown node mode if it was running
    if (mode_initialized) {
      // Node mode doesn't need explicit shutdown
    }
    
    // Initialize standalone mode
    standalone.begin(&timing);
    
    // Status indication
    DEBUG_PRINTLN("=== WIFI/LITE MODE ACTIVE ===");
    DEBUG_PRINTLN("Connect to WiFi: RaceTimer-XXXX");
    DEBUG_PRINTLN("Web interface: http://192.168.4.1");
    DEBUG_PRINTLN("ESP32-C3 Single-core operation");
    
  } else {
    // NODE MODE: NO debug output - it interferes with binary serial protocol
    
    // Shutdown standalone mode if it was running  
    if (mode_initialized) {
      // Standalone mode doesn't need explicit shutdown
    }
    
    // Initialize node mode
    node.begin(&timing);
    
    // Node mode is now active and waiting for RotorHazard commands
    // All communication is binary - no text output allowed
  }
  
  mode_initialized = true;
  
  // Flash LED to indicate mode change
  for (int i = 0; i < (current_mode == MODE_STANDALONE ? 3 : 2); i++) {
    digitalWrite(STATUS_LED_PIN, HIGH);
    delay(150);
    digitalWrite(STATUS_LED_PIN, LOW);
    delay(150);
  }
}
