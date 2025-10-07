#ifndef STANDALONE_MODE_H
#define STANDALONE_MODE_H

#include <Arduino.h>
#include <WiFi.h>
#include <WebServer.h> // Standard ESP32 WebServer (synchronous)
#include <SPIFFS.h>
#include <vector>
#include <U8g2lib.h>
#include "timing_core.h" // To interact with timing data
#include "config.h"

class StandaloneMode {
public:
    StandaloneMode();
    void begin(TimingCore* timingCore);
    void process();

private:
    WebServer _server;
    TimingCore* _timingCore;
    std::vector<LapData> _laps;
    bool _raceActive = false;
    uint32_t _raceStartTime = 0;
    
    // Display support
    U8G2_SSD1306_128X64_NONAME_F_SW_I2C _display;
    bool _displayConnected = false;
    uint32_t _lastDisplayUpdate = 0;

    void handleRoot();
    void handleGetStatus();
    void handleGetLaps();
    void handleStartRace();
    void handleStopRace();
    void handleClearLaps();
    void handleSetFrequency();
    void handleSetThreshold();
    void handleGetChannels();
    void handleStyleCSS();
    void handleAppJS();
    void handleNotFound();
    void setupWiFiAP();
    void initDisplay();
    void updateDisplay();
};

#endif // STANDALONE_MODE_H