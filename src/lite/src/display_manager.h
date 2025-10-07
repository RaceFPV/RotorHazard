#ifndef DISPLAY_MANAGER_H
#define DISPLAY_MANAGER_H

#include <Arduino.h>
#include <U8g2lib.h>
#include "config.h"

class DisplayManager {
public:
    DisplayManager();
    bool begin();
    void update();
    
    // Display modes
    void showReady(uint16_t frequency, uint8_t rssi, uint8_t threshold, bool crossing);
    void showRaceActive(uint8_t lapCount, uint32_t currentLapTime, uint32_t bestLapTime);
    void showInitializing();
    void showError(const char* message);
    
    // Configuration
    void setUpdateInterval(uint32_t intervalMs) { _updateInterval = intervalMs; }
    bool isConnected() const { return _connected; }
    
private:
    U8G2_SSD1306_128X64_NONAME_F_SW_I2C _display;
    bool _connected;
    uint32_t _lastUpdate;
    uint32_t _updateInterval;
    
    // Display state
    uint16_t _frequency;
    uint8_t _rssi;
    uint8_t _threshold;
    bool _crossing;
    uint8_t _lapCount;
    uint32_t _currentLapTime;
    uint32_t _bestLapTime;
    bool _raceActive;
    bool _displayError;
    
    // Helper functions
    void drawHeader(const char* title);
    void drawRSSI(uint8_t rssi, uint8_t threshold);
    void drawFrequency(uint16_t frequency);
    void drawCrossingIndicator();
    void drawLapInfo(uint8_t lapCount, uint32_t currentLapTime, uint32_t bestLapTime);
    void drawStatus(const char* status);
    void formatTime(uint32_t ms, char* buffer, size_t bufferSize);
};

#endif // DISPLAY_MANAGER_H
