// Race Timer JavaScript
class RaceTimer {
    constructor() {
        this.socket = null;
        this.isConnected = false;
        this.raceActive = false;
        this.laps = [];
        this.currentStatus = {};
        
        this.initializeUI();
        this.connectWebSocket();
    }
    
    initializeUI() {
        // Initialize threshold slider
        const thresholdSlider = document.getElementById('threshold');
        const thresholdValue = document.getElementById('thresholdValue');
        
        thresholdSlider.addEventListener('input', (e) => {
            thresholdValue.textContent = e.target.value;
            this.updateThresholdLine(e.target.value);
        });
        
        // Initialize threshold line position
        this.updateThresholdLine(thresholdSlider.value);
    }
    
    connectWebSocket() {
        const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const wsUrl = `${protocol}//${window.location.host}/ws`;
        
        console.log('Connecting to WebSocket:', wsUrl);
        
        this.socket = new WebSocket(wsUrl);
        
        this.socket.onopen = () => {
            console.log('WebSocket connected');
            this.setConnectionStatus(true);
            this.requestInitialData();
        };
        
        this.socket.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data);
                this.handleMessage(data);
            } catch (e) {
                console.error('Error parsing WebSocket message:', e);
            }
        };
        
        this.socket.onclose = () => {
            console.log('WebSocket disconnected');
            this.setConnectionStatus(false);
            // Attempt to reconnect after 3 seconds
            setTimeout(() => this.connectWebSocket(), 3000);
        };
        
        this.socket.onerror = (error) => {
            console.error('WebSocket error:', error);
            this.setConnectionStatus(false);
        };
    }
    
    setConnectionStatus(connected) {
        this.isConnected = connected;
        const statusDot = document.getElementById('statusDot');
        const statusText = document.getElementById('statusText');
        
        if (connected) {
            statusDot.className = 'status-dot connected';
            statusText.textContent = 'Connected';
        } else {
            statusDot.className = 'status-dot connecting';
            statusText.textContent = 'Connecting...';
        }
    }
    
    handleMessage(data) {
        switch (data.type) {
            case 'status':
                this.updateStatus(data);
                break;
            case 'lap':
                this.addLap(data);
                break;
            case 'race_state':
                this.updateRaceState(data);
                break;
            default:
                console.log('Unknown message type:', data.type);
        }
    }
    
    updateStatus(data) {
        this.currentStatus = data;
        
        // Update RSSI displays
        document.getElementById('currentRssi').textContent = data.current_rssi || 0;
        document.getElementById('peakRssi').textContent = data.peak_rssi || 0;
        document.getElementById('lapCount').textContent = data.lap_count || 0;
        
        // Update RSSI bar
        const rssiPercent = Math.min((data.current_rssi || 0) / 255 * 100, 100);
        document.getElementById('rssiFill').style.width = rssiPercent + '%';
        
        // Show crossing state
        const statusItems = document.querySelectorAll('.status-item');
        if (data.crossing) {
            statusItems.forEach(item => item.classList.add('crossing'));
        } else {
            statusItems.forEach(item => item.classList.remove('crossing'));
        }
    }
    
    addLap(lapData) {
        this.laps.push(lapData);
        this.updateLapDisplay();
        this.updateLastLap(lapData);
        
        // Flash animation for new lap
        setTimeout(() => {
            const lapItems = document.querySelectorAll('.lap-item');
            if (lapItems.length > 0) {
                lapItems[0].classList.add('new-lap');
                setTimeout(() => {
                    lapItems[0].classList.remove('new-lap');
                }, 1000);
            }
        }, 100);
    }
    
    updateLastLap(lapData) {
        const lastLapElement = document.getElementById('lastLap');
        if (lapData.lap_time_ms > 0) {
            lastLapElement.textContent = this.formatTime(lapData.lap_time_ms);
        }
    }
    
    updateLapDisplay() {
        const lapList = document.getElementById('lapList');
        
        if (this.laps.length === 0) {
            lapList.innerHTML = '<div class="no-laps">No laps recorded yet</div>';
            return;
        }
        
        // Find best lap
        const validLaps = this.laps.filter(lap => lap.lap_time_ms > 0);
        const bestLap = validLaps.length > 0 ? 
            Math.min(...validLaps.map(lap => lap.lap_time_ms)) : null;
        
        lapList.innerHTML = this.laps.slice().reverse().map((lap, index) => {
            const lapNumber = this.laps.length - index;
            const isBest = lap.lap_time_ms === bestLap && lap.lap_time_ms > 0;
            const lapTime = lap.lap_time_ms > 0 ? this.formatTime(lap.lap_time_ms) : '--:--';
            
            return `
                <div class="lap-item ${isBest ? 'best' : ''}">
                    <div>
                        <div class="lap-number">Lap ${lapNumber}</div>
                        <div class="lap-info">Peak: ${lap.rssi_peak}</div>
                    </div>
                    <div class="lap-time">${lapTime}</div>
                </div>
            `;
        }).join('');
    }
    
    updateRaceState(data) {
        this.raceActive = data.active;
        
        const startBtn = document.getElementById('startBtn');
        const stopBtn = document.getElementById('stopBtn');
        
        startBtn.disabled = this.raceActive;
        stopBtn.disabled = !this.raceActive;
    }
    
    updateThresholdLine(threshold) {
        const thresholdLine = document.getElementById('thresholdLine');
        const percent = Math.min(threshold / 255 * 100, 100);
        thresholdLine.style.left = percent + '%';
    }
    
    formatTime(milliseconds) {
        const totalSeconds = Math.floor(milliseconds / 1000);
        const minutes = Math.floor(totalSeconds / 60);
        const seconds = totalSeconds % 60;
        const ms = Math.floor((milliseconds % 1000) / 10);
        
        return `${minutes}:${seconds.toString().padStart(2, '0')}.${ms.toString().padStart(2, '0')}`;
    }
    
    sendCommand(command, data = {}) {
        if (this.socket && this.socket.readyState === WebSocket.OPEN) {
            this.socket.send(JSON.stringify({
                command: command,
                ...data
            }));
        } else {
            console.warn('WebSocket not connected, cannot send command:', command);
        }
    }
    
    requestInitialData() {
        this.sendCommand('get_status');
        this.sendCommand('get_laps');
    }
}

// Global functions for button handlers
let raceTimer;

function startRace() {
    raceTimer.sendCommand('start_race');
}

function stopRace() {
    raceTimer.sendCommand('stop_race');
}

function resetRace() {
    if (confirm('Are you sure you want to reset all lap data?')) {
        raceTimer.sendCommand('reset_race');
        raceTimer.laps = [];
        raceTimer.updateLapDisplay();
    }
}

function updateFrequency() {
    const frequency = document.getElementById('frequency').value;
    raceTimer.sendCommand('set_frequency', { frequency: parseInt(frequency) });
}

function updateThreshold() {
    const threshold = document.getElementById('threshold').value;
    raceTimer.sendCommand('set_threshold', { threshold: parseInt(threshold) });
    raceTimer.updateThresholdLine(threshold);
}

// Initialize when page loads
document.addEventListener('DOMContentLoaded', () => {
    raceTimer = new RaceTimer();
});

// Handle page visibility changes
document.addEventListener('visibilitychange', () => {
    if (!document.hidden && raceTimer) {
        // Page became visible, refresh data
        raceTimer.requestInitialData();
    }
});
