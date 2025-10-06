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

void setup() {
  Serial.begin(UART_BAUD_RATE);
  
  // Wait for serial connection (optional)
  #ifdef ARDUINO_USB_CDC_ON_BOOT
  delay(2000); // Give time for USB serial to connect
  #endif
  
  DEBUG_PRINTLN("=== RotorHazard Lite ESP32-C3 Timer ===");
  DEBUG_PRINTLN("Version: 1.0.0");
  DEBUG_PRINTLN("Single-core RISC-V processor");
  
  // Initialize mode selection pin (GND=Node, 3.3V=WiFi)
  pinMode(MODE_SWITCH_PIN, INPUT);
  pinMode(STATUS_LED_PIN, OUTPUT);
  
  // Brief startup flash
  digitalWrite(STATUS_LED_PIN, HIGH);
  delay(500);
  digitalWrite(STATUS_LED_PIN, LOW);
  delay(200);
  
  // Initialize core timing system (always active)
  DEBUG_PRINTLN("Initializing timing core...");
  timing.begin();
  
  // Determine initial mode
  checkModeSwitch();
  initializeMode();
  
  DEBUG_PRINTLN("Setup complete!");
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
  
  // Brief yield to prevent watchdog issues (ESP32-C3 single core)
  vTaskDelay(pdMS_TO_TICKS(10));
}

void checkModeSwitch() {
  static unsigned long last_check = 0;
  static bool last_switch_state = HIGH;
  
  // Only check every 100ms to avoid bouncing
  if (millis() - last_check < 100) {
    return;
  }
  
  bool current_switch_state = digitalRead(MODE_SWITCH_PIN);
  
  if (current_switch_state != last_switch_state) {
    DEBUG_PRINT("Mode switch changed: ");
    DEBUG_PRINTLN(current_switch_state == WIFI_MODE ? "WIFI" : "ROTORHAZARD");
    
    // Determine new mode
    OperationMode new_mode = (current_switch_state == WIFI_MODE) ? 
                             MODE_STANDALONE : MODE_ROTORHAZARD;
    
    if (new_mode != current_mode || !mode_initialized) {
      current_mode = new_mode;
      initializeMode();
    }
    
    last_switch_state = current_switch_state;
  }
  
  last_check = millis();
}

void initializeMode() {
  DEBUG_PRINT("Initializing mode: ");
  
  if (current_mode == MODE_STANDALONE) {
    DEBUG_PRINTLN("STANDALONE/WIFI");
    
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
    DEBUG_PRINTLN("ROTORHAZARD NODE");
    
    // Shutdown standalone mode if it was running  
    if (mode_initialized) {
      // Standalone mode doesn't need explicit shutdown
    }
    
    // Initialize node mode
    node.begin(&timing);
    
    // Status indication
    DEBUG_PRINTLN("=== ROTORHAZARD NODE MODE ACTIVE ===");
    DEBUG_PRINTLN("Waiting for RotorHazard connection...");
    DEBUG_PRINT("UART: ");
    DEBUG_PRINT(UART_BAUD_RATE);
    DEBUG_PRINTLN(" baud");
    DEBUG_PRINTLN("ESP32-C3 Single-core operation");
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
