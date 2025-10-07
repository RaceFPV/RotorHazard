#include "display_manager.h"
#include "display_config.h"

DisplayManager::DisplayManager() 
    : _display(U8G2_R0, DISPLAY_SCL_PIN, DISPLAY_SDA_PIN, U8X8_PIN_NONE),
      _connected(false),
      _lastUpdate(0),
      _updateInterval(DISPLAY_UPDATE_INTERVAL_MS),
      _frequency(5800),
      _rssi(0),
      _threshold(50),
      _crossing(false),
      _lapCount(0),
      _currentLapTime(0),
      _bestLapTime(0),
      _raceActive(false),
      _displayError(false) {
}

bool DisplayManager::begin() {
    // Display disabled to preserve timing core performance
    //_connected = false;
    //Serial.println("Display disabled to preserve timing performance");
    //return false;
    
    if (_display.begin()) {
        _connected = true;
        
        // Use the most basic font possible to avoid crashes
        _display.clearBuffer();
        _display.setFont(u8g2_font_5x7_tr);
        _display.drawStr(0, 10, "RotorHazard");
        _display.drawStr(0, 25, "Init...");
        _display.sendBuffer();
        
        Serial.println("Display initialized successfully");
        return true;
    } else {
        _connected = false;
        Serial.println("Display not connected or failed to initialize");
        return false;
    }
}

void DisplayManager::update() {
    if (!_connected || _displayError) return;
    
    uint32_t now = millis();
    uint32_t interval = _raceActive ? DISPLAY_RACE_UPDATE_MS : _updateInterval;
    if (now - _lastUpdate < interval) return;
    
    // Safety check - prevent too frequent updates
    if (now - _lastUpdate < 50) return;
    
    // Try to update display with error handling
    try {
        _display.clearBuffer();
        
        // Use the most basic font - try different approach
        _display.setFont(u8g2_font_5x7_tr);
        
        if (_raceActive) {
            showRaceActive(_lapCount, _currentLapTime, _bestLapTime);
        } else {
            showReady(_frequency, _rssi, _threshold, _crossing);
        }
        
        _display.sendBuffer();
        _lastUpdate = now;
    } catch (...) {
        // If display operations fail, disable display
        _displayError = true;
        _connected = false;
        Serial.println("Display error - disabling display");
    }
}

void DisplayManager::showReady(uint16_t frequency, uint8_t rssi, uint8_t threshold, bool crossing) {
    _frequency = frequency;
    _rssi = rssi;
    _threshold = threshold;
    _crossing = crossing;
    _raceActive = false;
    
    // Simple, safe display for ready state - add 2px offset to prevent cut-off
    _display.drawStr(2, 10, "RotorHazard");
    _display.drawStr(2, 25, "Ready");
    
    // Only show basic info to avoid crashes
    char freqText[15];
    snprintf(freqText, sizeof(freqText), "F:%d", frequency);
    _display.drawStr(2, 40, freqText);
    
    char rssiText[15];
    snprintf(rssiText, sizeof(rssiText), "R:%d", rssi);
    _display.drawStr(2, 55, rssiText);
    
    if (crossing) {
        _display.drawStr(82, 25, "CROSS!");
    }
}

void DisplayManager::showRaceActive(uint8_t lapCount, uint32_t currentLapTime, uint32_t bestLapTime) {
    _lapCount = lapCount;
    _currentLapTime = currentLapTime;
    _bestLapTime = bestLapTime;
    _raceActive = true;
    
    // Simple, safe display for race state - add 2px offset to prevent cut-off
    _display.drawStr(2, 10, "RACE ACTIVE");
    
    char lapText[15];
    snprintf(lapText, sizeof(lapText), "Laps: %d", lapCount);
    _display.drawStr(2, 25, lapText);
    
    if (currentLapTime > 0) {
        char timeText[15];
        snprintf(timeText, sizeof(timeText), "L: %d.%03d", 
                (int)(currentLapTime / 1000), (int)(currentLapTime % 1000));
        _display.drawStr(2, 40, timeText);
    }
    
    if (bestLapTime > 0) {
        char bestText[15];
        snprintf(bestText, sizeof(bestText), "B: %d.%03d", 
                (int)(bestLapTime / 1000), (int)(bestLapTime % 1000));
        _display.drawStr(2, 55, bestText);
    }
}

void DisplayManager::showInitializing() {
    // Simple initialization display - minimal font usage - add 2px offset
    _display.drawStr(2, 10, "RotorHazard");
    _display.drawStr(2, 25, "Init...");
}

void DisplayManager::showError(const char* message) {
    drawHeader("ERROR");
    drawStatus(message);
}

void DisplayManager::drawHeader(const char* title) {
    if (title && strlen(title) > 0) {
        _display.drawStr(0, DISPLAY_Y_TITLE, title);
    }
}

void DisplayManager::drawRSSI(uint8_t rssi, uint8_t threshold) {
    char rssiText[20];
    snprintf(rssiText, sizeof(rssiText), "RSSI: %d/%d", rssi, threshold);
    _display.drawStr(0, DISPLAY_Y_RSSI, rssiText);
    
    // Visual RSSI bar (simplified)
    int barWidth = (rssi * DISPLAY_RSSI_BAR_MAX_WIDTH) / 255; // Convert to percentage
    if (barWidth > DISPLAY_RSSI_BAR_MAX_WIDTH) barWidth = DISPLAY_RSSI_BAR_MAX_WIDTH;
    _display.drawHLine(0, DISPLAY_RSSI_BAR_Y, barWidth);
}

void DisplayManager::drawFrequency(uint16_t frequency) {
    char freqText[20];
    snprintf(freqText, sizeof(freqText), "Freq: %dMHz", frequency);
    _display.drawStr(0, DISPLAY_Y_FREQUENCY, freqText);
}

void DisplayManager::drawCrossingIndicator() {
    _display.drawStr(DISPLAY_X_CROSSING, DISPLAY_Y_CROSSING, DISPLAY_CROSSING_TEXT);
}

void DisplayManager::drawLapInfo(uint8_t lapCount, uint32_t currentLapTime, uint32_t bestLapTime) {
    // Lap count
    char lapText[20];
    snprintf(lapText, sizeof(lapText), "Laps: %d", lapCount);
    _display.drawStr(0, DISPLAY_Y_LAP_COUNT, lapText);
    
    // Current lap time
    if (currentLapTime > 0) {
        char timeText[20];
        formatTime(currentLapTime, timeText, sizeof(timeText));
        char currentText[25];
        snprintf(currentText, sizeof(currentText), "Lap: %s", timeText);
        _display.drawStr(0, DISPLAY_Y_CURRENT_LAP, currentText);
    }
    
    // Best lap time
    #if DISPLAY_SHOW_BEST_LAP
        if (bestLapTime > 0) {
            char timeText[20];
            formatTime(bestLapTime, timeText, sizeof(timeText));
            char bestText[25];
            snprintf(bestText, sizeof(bestText), "Best: %s", timeText);
            _display.drawStr(0, DISPLAY_Y_BEST_LAP, bestText);
        }
    #endif
}

void DisplayManager::drawStatus(const char* status) {
    _display.drawStr(0, DISPLAY_Y_STATUS, status);
}

void DisplayManager::formatTime(uint32_t ms, char* buffer, size_t bufferSize) {
    uint32_t seconds = ms / 1000;
    uint32_t milliseconds = ms % 1000;
    
    if (seconds >= 60) {
        uint32_t minutes = seconds / 60;
        seconds = seconds % 60;
        snprintf(buffer, bufferSize, "%d:%02d.%03d", minutes, seconds, milliseconds);
    } else {
        snprintf(buffer, bufferSize, "%d.%03ds", seconds, milliseconds);
    }
}
