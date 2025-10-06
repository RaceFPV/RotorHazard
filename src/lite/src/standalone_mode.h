#ifndef STANDALONE_MODE_H
#define STANDALONE_MODE_H

#include <Arduino.h>
#include <WebServer.h> // For ESP32 WebServer
#include <vector>
#include "timing_core.h" // To interact with timing data

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

    void handleRoot();
    void handleGetStatus();
    void handleGetLaps();
    void handleStartRace();
    void handleStopRace();
    void handleClearLaps();
    void handleStyleCSS();
    void handleAppJS();
    void handleNotFound();
    void setupWiFiAP();
};

#endif // STANDALONE_MODE_H