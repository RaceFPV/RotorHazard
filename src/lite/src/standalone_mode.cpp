#include "standalone_mode.h"
#include <WiFi.h>
#include <SPIFFS.h>
#include "config.h"

StandaloneMode::StandaloneMode() : _server(80), _timingCore(nullptr) {
}

void StandaloneMode::begin(TimingCore* timingCore) {
    _timingCore = timingCore;
    
    // Initialize WiFi AP
    setupWiFiAP();
    
    // Initialize SPIFFS for serving static files
    if (!SPIFFS.begin(true)) {
        Serial.println("SPIFFS Mount Failed");
        return;
    }
    
    // Setup web server routes
    _server.on("/", HTTP_GET, [this]() { handleRoot(); });
    _server.on("/api/status", HTTP_GET, [this]() { handleGetStatus(); });
    _server.on("/api/laps", HTTP_GET, [this]() { handleGetLaps(); });
    _server.on("/api/start_race", HTTP_POST, [this]() { handleStartRace(); });
    _server.on("/api/stop_race", HTTP_POST, [this]() { handleStopRace(); });
    _server.on("/api/clear_laps", HTTP_POST, [this]() { handleClearLaps(); });
    _server.on("/style.css", HTTP_GET, [this]() { handleStyleCSS(); });
    _server.on("/app.js", HTTP_GET, [this]() { handleAppJS(); });
    _server.onNotFound([this]() { handleNotFound(); });
    
    _server.begin();
    Serial.println("Web server started");
    Serial.printf("Access point: %s\n", ssid);
    Serial.printf("IP address: %s\n", WiFi.softAPIP().toString().c_str());
    Serial.println("Open browser to http://192.168.4.1");
}

void StandaloneMode::process() {
    _server.handleClient();
    
    // Check for new lap data
    if (_timingCore && _timingCore->hasNewLap()) {
        LapData lap = _timingCore->getNextLap();
        
        // Store lap in our internal list (simple vector)
        _laps.push_back(lap);
        
        // Limit stored laps to prevent memory issues
        if (_laps.size() > 100) {
            _laps.erase(_laps.begin());
        }
        
        Serial.printf("Lap recorded: %dms, RSSI: %d\n", lap.timestamp_ms, lap.rssi_peak);
    }
}

void StandaloneMode::setupWiFiAP() {
    WiFi.mode(WIFI_AP);
    
    // Create unique SSID with MAC address
    String macAddr = WiFi.macAddress();
    macAddr.replace(":", "");
    String apSSID = String(ssid) + "-" + macAddr.substring(8);
    
    WiFi.softAP(apSSID.c_str(), password);
    
    // Configure IP
    IPAddress local_IP(192, 168, 4, 1);
    IPAddress gateway(192, 168, 4, 1);
    IPAddress subnet(255, 255, 255, 0);
    WiFi.softAPConfig(local_IP, gateway, subnet);
    
    Serial.printf("WiFi AP started: %s\n", apSSID.c_str());
}

void StandaloneMode::handleRoot() {
    String html = R"(
<!DOCTYPE html>
<html>
<head>
    <title>ESP32 Race Timer</title>
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <link rel="stylesheet" href="/style.css">
</head>
<body>
    <div class="container">
        <h1>🏁 Race Timer Lite</h1>
        <div id="status" class="status">Status: Ready</div>
        
        <div class="controls">
            <button id="startBtn" onclick="startRace()" class="btn btn-primary">Start Race</button>
            <button id="stopBtn" onclick="stopRace()" class="btn btn-secondary">Stop Race</button>
            <button id="clearBtn" onclick="clearLaps()" class="btn btn-danger">Clear Laps</button>
        </div>
        
        <div class="stats">
            <div class="stat-card">
                <div class="stat-number" id="lapCount">0</div>
                <div class="stat-label">Total Laps</div>
            </div>
            <div class="stat-card">
                <div class="stat-number" id="bestLap">--:--</div>
                <div class="stat-label">Best Lap</div>
            </div>
            <div class="stat-card">
                <div class="stat-number" id="lastLap">--:--</div>
                <div class="stat-label">Last Lap</div>
            </div>
        </div>
        
        <div class="laps-section">
            <h2>Lap Times</h2>
            <div id="laps" class="laps-container">
                <p class="no-laps">No laps recorded yet</p>
            </div>
        </div>
    </div>
    <script src="/app.js"></script>
</body>
</html>
)";
    _server.send(200, "text/html", html);
}

void StandaloneMode::handleGetStatus() {
    String json = "{";
    json += "\"status\":\"" + String(_raceActive ? "racing" : "ready") + "\",";
    json += "\"lap_count\":" + String(_laps.size()) + ",";
    json += "\"uptime\":" + String(millis()) + ",";
    json += "\"rssi\":" + String(_timingCore ? _timingCore->getCurrentRSSI() : 0);
    json += "}";
    
    _server.send(200, "application/json", json);
}

void StandaloneMode::handleGetLaps() {
    String json = "[";
    
    for (size_t i = 0; i < _laps.size(); i++) {
        if (i > 0) json += ",";
        json += "{";
        json += "\"lap_number\":" + String(i + 1) + ",";
        json += "\"timestamp_ms\":" + String(_laps[i].timestamp_ms) + ",";
        json += "\"peak_rssi\":" + String(_laps[i].rssi_peak) + ",";
        
        // Calculate lap time (difference from previous lap or start)
        uint32_t lapTime = 0;
        if (i == 0) {
            lapTime = _laps[i].timestamp_ms - _raceStartTime;
        } else {
            lapTime = _laps[i].timestamp_ms - _laps[i-1].timestamp_ms;
        }
        json += "\"lap_time_ms\":" + String(lapTime);
        json += "}";
    }
    
    json += "]";
    _server.send(200, "application/json", json);
}

void StandaloneMode::handleStartRace() {
    _raceActive = true;
    _raceStartTime = millis();
    _laps.clear();
    
    Serial.println("Race started!");
    _server.send(200, "application/json", "{\"status\":\"race_started\"}");
}

void StandaloneMode::handleStopRace() {
    _raceActive = false;
    
    Serial.println("Race stopped!");
    _server.send(200, "application/json", "{\"status\":\"race_stopped\"}");
}

void StandaloneMode::handleClearLaps() {
    _laps.clear();
    
    Serial.println("Laps cleared!");
    _server.send(200, "application/json", "{\"status\":\"laps_cleared\"}");
}

void StandaloneMode::handleStyleCSS() {
    String css = R"(
body {
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    margin: 0;
    padding: 20px;
    background: linear-gradient(135deg, #667eea 0%, #764ba2 100%);
    min-height: 100vh;
    color: #333;
}

.container {
    max-width: 800px;
    margin: 0 auto;
    background: white;
    border-radius: 12px;
    box-shadow: 0 8px 32px rgba(0,0,0,0.1);
    padding: 30px;
}

h1 {
    text-align: center;
    color: #2c3e50;
    margin-bottom: 30px;
    font-size: 2.5em;
    font-weight: 700;
}

.status {
    background: #f8f9fa;
    padding: 15px;
    border-radius: 8px;
    margin-bottom: 25px;
    text-align: center;
    font-weight: 600;
    border-left: 4px solid #007bff;
}

.controls {
    display: flex;
    gap: 15px;
    justify-content: center;
    margin-bottom: 30px;
    flex-wrap: wrap;
}

.btn {
    padding: 12px 24px;
    border: none;
    border-radius: 6px;
    font-size: 16px;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.2s;
    min-width: 120px;
}

.btn-primary {
    background: #28a745;
    color: white;
}

.btn-primary:hover {
    background: #218838;
    transform: translateY(-1px);
}

.btn-secondary {
    background: #6c757d;
    color: white;
}

.btn-secondary:hover {
    background: #5a6268;
    transform: translateY(-1px);
}

.btn-danger {
    background: #dc3545;
    color: white;
}

.btn-danger:hover {
    background: #c82333;
    transform: translateY(-1px);
}

.stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 20px;
    margin-bottom: 30px;
}

.stat-card {
    background: #f8f9fa;
    padding: 20px;
    border-radius: 8px;
    text-align: center;
    border: 1px solid #e9ecef;
}

.stat-number {
    font-size: 2em;
    font-weight: 700;
    color: #007bff;
    margin-bottom: 5px;
}

.stat-label {
    color: #6c757d;
    font-size: 0.9em;
    text-transform: uppercase;
    letter-spacing: 0.5px;
}

.laps-section h2 {
    color: #2c3e50;
    margin-bottom: 20px;
    padding-bottom: 10px;
    border-bottom: 2px solid #e9ecef;
}

.laps-container {
    max-height: 400px;
    overflow-y: auto;
}

.lap-item {
    background: #f8f9fa;
    margin-bottom: 10px;
    padding: 15px;
    border-radius: 6px;
    border-left: 4px solid #007bff;
    display: flex;
    justify-content: space-between;
    align-items: center;
}

.lap-number {
    font-weight: 700;
    color: #007bff;
}

.lap-time {
    font-weight: 600;
    font-family: 'Courier New', monospace;
}

.lap-rssi {
    color: #6c757d;
    font-size: 0.9em;
}

.no-laps {
    text-align: center;
    color: #6c757d;
    font-style: italic;
    padding: 40px;
}

@media (max-width: 600px) {
    .container {
        padding: 20px;
        margin: 10px;
    }
    
    .controls {
        flex-direction: column;
        align-items: center;
    }
    
    .btn {
        width: 100%;
        max-width: 200px;
    }
}
)";
    _server.send(200, "text/css", css);
}

void StandaloneMode::handleAppJS() {
    String js = R"(
let raceActive = false;
let updateInterval;

document.addEventListener('DOMContentLoaded', () => {
    updateData();
    startPeriodicUpdates();
});

function startPeriodicUpdates() {
    updateInterval = setInterval(updateData, 1000);
}

async function updateData() {
    try {
        // Update status
        const statusResponse = await fetch('/api/status');
        const status = await statusResponse.json();
        
        document.getElementById('status').textContent = 
            `Status: ${status.status} | RSSI: ${status.rssi} | Uptime: ${formatTime(status.uptime)}`;
        
        // Update laps
        const lapsResponse = await fetch('/api/laps');
        const laps = await lapsResponse.json();
        
        updateLapsDisplay(laps);
        updateStats(laps);
        
    } catch (error) {
        console.error('Error updating data:', error);
        document.getElementById('status').textContent = 'Status: Connection Error';
    }
}

function updateLapsDisplay(laps) {
    const lapsContainer = document.getElementById('laps');
    
    if (laps.length === 0) {
        lapsContainer.innerHTML = '<p class="no-laps">No laps recorded yet</p>';
        return;
    }
    
    let html = '';
    laps.forEach((lap, index) => {
        html += `
            <div class="lap-item">
                <div class="lap-number">Lap ${lap.lap_number}</div>
                <div class="lap-time">${formatLapTime(lap.lap_time_ms)}</div>
                <div class="lap-rssi">RSSI: ${lap.peak_rssi}</div>
            </div>
        `;
    });
    
    lapsContainer.innerHTML = html;
}

function updateStats(laps) {
    document.getElementById('lapCount').textContent = laps.length;
    
    if (laps.length === 0) {
        document.getElementById('bestLap').textContent = '--:--';
        document.getElementById('lastLap').textContent = '--:--';
        return;
    }
    
    // Find best lap time
    let bestTime = Math.min(...laps.map(lap => lap.lap_time_ms));
    document.getElementById('bestLap').textContent = formatLapTime(bestTime);
    
    // Show last lap time
    let lastTime = laps[laps.length - 1].lap_time_ms;
    document.getElementById('lastLap').textContent = formatLapTime(lastTime);
}

async function startRace() {
    try {
        const response = await fetch('/api/start_race', { method: 'POST' });
        if (response.ok) {
            raceActive = true;
            updateData();
        }
    } catch (error) {
        console.error('Error starting race:', error);
    }
}

async function stopRace() {
    try {
        const response = await fetch('/api/stop_race', { method: 'POST' });
        if (response.ok) {
            raceActive = false;
            updateData();
        }
    } catch (error) {
        console.error('Error stopping race:', error);
    }
}

async function clearLaps() {
    if (confirm('Clear all lap data?')) {
        try {
            const response = await fetch('/api/clear_laps', { method: 'POST' });
            if (response.ok) {
                updateData();
            }
        } catch (error) {
            console.error('Error clearing laps:', error);
        }
    }
}

function formatTime(ms) {
    const seconds = Math.floor(ms / 1000);
    const minutes = Math.floor(seconds / 60);
    const hours = Math.floor(minutes / 60);
    
    if (hours > 0) {
        return `${hours}:${(minutes % 60).toString().padStart(2, '0')}:${(seconds % 60).toString().padStart(2, '0')}`;
    } else if (minutes > 0) {
        return `${minutes}:${(seconds % 60).toString().padStart(2, '0')}`;
    } else {
        return `${seconds}s`;
    }
}

function formatLapTime(ms) {
    const totalSeconds = ms / 1000;
    const minutes = Math.floor(totalSeconds / 60);
    const seconds = (totalSeconds % 60).toFixed(3);
    
    if (minutes > 0) {
        return `${minutes}:${seconds.padStart(6, '0')}`;
    } else {
        return `${seconds}s`;
    }
}
)";
    _server.send(200, "application/javascript", js);
}

void StandaloneMode::handleNotFound() {
    _server.send(404, "text/plain", "File not found");
}
