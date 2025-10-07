#include "timing_core.h"
#include <SPI.h>

// RX5808 register definitions (from original RotorHazard node code)
#define RX5808_WRITE_REGISTER   0x00
#define RX5808_SYNTH_A_REGISTER 0x01
#define RX5808_SYNTH_B_REGISTER 0x02

TimingCore::TimingCore() {
  // Initialize state
  memset(&state, 0, sizeof(state));
  state.threshold = CROSSING_THRESHOLD;
  state.frequency_mhz = DEFAULT_FREQ;
  
  // Initialize buffers
  memset(lap_buffer, 0, sizeof(lap_buffer));
  memset(rssi_samples, 0, sizeof(rssi_samples));
  
  lap_write_index = 0;
  lap_read_index = 0;
  sample_index = 0;
  samples_filled = false;
  
  // Initialize FreeRTOS objects
  timing_task_handle = nullptr;
  timing_mutex = xSemaphoreCreateMutex();
}

void TimingCore::begin() {
  Serial.println("TimingCore: Initializing...");
  
  // Setup pins
  pinMode(RSSI_INPUT_PIN, INPUT);
  pinMode(STATUS_LED_PIN, OUTPUT);
  
  // Test ADC reading immediately
  uint16_t test_adc = analogRead(RSSI_INPUT_PIN);
  Serial.printf("ADC test reading on pin %d: %d (raw 12-bit)\n", RSSI_INPUT_PIN, test_adc);
  Serial.printf("Converted to 8-bit: %d\n", (test_adc >> 4) & 0xFF);
  
  // Initialize RX5808 module
  setupRX5808();
  
  // Set default frequency
  setRX5808Frequency(state.frequency_mhz);
  
  // Initialize RSSI filtering with some test readings
  for (int i = 0; i < RSSI_SAMPLES; i++) {
    uint16_t raw_adc = analogRead(RSSI_INPUT_PIN);
    rssi_samples[i] = (raw_adc >> 4) & 0xFF; // Convert 12-bit to 8-bit
    Serial.printf("Initial RSSI sample %d: ADC=%d, 8-bit=%d\n", i, raw_adc, rssi_samples[i]);
  }
  
  // Create timing task for ESP32-C3 single core (high priority)
  xTaskCreate(timingTask, "TimingTask", 4096, this, TIMING_PRIORITY, &timing_task_handle);
  
  // Mark as activated
  state.activated = true;
  
  Serial.println("TimingCore: Ready");
}

void TimingCore::process() {
  // For ESP32-C3, timing is handled by the dedicated task
  // This method is kept for compatibility but does minimal work
  if (!state.activated) {
    return;
  }
  
  // Just yield to allow other tasks to run
  vTaskDelay(pdMS_TO_TICKS(1));
}

// FreeRTOS task for timing processing (ESP32-C3 single core)
void TimingCore::timingTask(void* parameter) {
  TimingCore* core = static_cast<TimingCore*>(parameter);
  uint32_t debug_counter = 0;
  
  while (true) {
    if (!core->state.activated) {
      vTaskDelay(pdMS_TO_TICKS(100));
      continue;
    }
    
    static uint32_t last_process_time = 0;
    uint32_t current_time = millis();
    
    // Limit processing to configured interval
    if (current_time - last_process_time < TIMING_INTERVAL_MS) {
      vTaskDelay(pdMS_TO_TICKS(1));
      continue;
    }
    
    // Take mutex for thread safety
    if (xSemaphoreTake(core->timing_mutex, portMAX_DELAY)) {
      // Read and filter RSSI
      uint8_t raw_rssi = core->readRawRSSI();
      uint8_t filtered_rssi = core->filterRSSI(raw_rssi);
      core->state.current_rssi = filtered_rssi;
      
      // Debug output every 1000 iterations (about once per second)
      debug_counter++;
      if (debug_counter % 1000 == 0) {
        Serial.printf("[TimingTask] Raw RSSI: %d, Filtered: %d, Threshold: %d\n", 
                      raw_rssi, filtered_rssi, core->state.threshold);
      }
      
      // Update peak tracking
      if (filtered_rssi > core->state.peak_rssi) {
        core->state.peak_rssi = filtered_rssi;
      }
      
      // Detect crossing events
      bool crossing_detected = core->detectCrossing(filtered_rssi);
      
      // Handle crossing state changes
      if (crossing_detected != core->state.crossing_active) {
        core->state.crossing_active = crossing_detected;
        
        if (crossing_detected) {
          // Starting a crossing
          core->state.crossing_start = current_time;
          Serial.printf("Crossing started - RSSI: %d\n", filtered_rssi);
        } else {
          // Ending a crossing - record lap
          uint32_t crossing_duration = current_time - core->state.crossing_start;
          if (crossing_duration > 100) { // Minimum 100ms crossing to avoid noise
            core->recordLap(current_time, core->state.peak_rssi);
          }
          Serial.printf("Crossing ended - Duration: %dms\n", crossing_duration);
        }
        
        // Notify callback if registered  
        if (core->crossing_callback) {
          core->crossing_callback(core->state.crossing_active, filtered_rssi);
        }
      }
      
      last_process_time = current_time;
      xSemaphoreGive(core->timing_mutex);
    }
    
    // Small delay to prevent task from consuming all CPU
    vTaskDelay(pdMS_TO_TICKS(1));
  }
}

uint8_t TimingCore::readRawRSSI() {
  // Read 12-bit ADC value and convert to 8-bit
  uint16_t adc_value = analogRead(RSSI_INPUT_PIN);
  return (adc_value >> 4) & 0xFF; // Convert 12-bit (0-4095) to 8-bit (0-255)
}

uint8_t TimingCore::filterRSSI(uint8_t raw_rssi) {
  // Simple moving average filter
  rssi_samples[sample_index] = raw_rssi;
  sample_index = (sample_index + 1) % RSSI_SAMPLES;
  
  if (!samples_filled && sample_index == 0) {
    samples_filled = true;
  }
  
  // Calculate average
  uint32_t sum = 0;
  uint8_t count = samples_filled ? RSSI_SAMPLES : sample_index;
  
  for (uint8_t i = 0; i < count; i++) {
    sum += rssi_samples[i];
  }
  
  return (count > 0) ? (sum / count) : raw_rssi;
}

bool TimingCore::detectCrossing(uint8_t filtered_rssi) {
  // Simple threshold-based crossing detection
  // TODO: Implement more sophisticated detection like original RotorHazard
  return filtered_rssi >= state.threshold;
}

void TimingCore::recordLap(uint32_t timestamp, uint8_t peak_rssi) {
  LapData& lap = lap_buffer[lap_write_index];
  
  lap.timestamp_ms = timestamp;
  lap.lap_time_ms = (state.last_lap_time > 0) ? 
                   (timestamp - state.last_lap_time) : 0;
  lap.rssi_peak = peak_rssi;
  lap.pilot_id = 0; // Single pilot for now
  lap.valid = true;
  
  // Update state
  state.last_lap_time = timestamp;
  state.lap_count++;
  
  // Advance write index
  lap_write_index = (lap_write_index + 1) % MAX_LAPS_STORED;
  
  // Reset peak tracking
  state.peak_rssi = 0;
  
        // Debug output disabled to avoid interfering with serial protocol
        // DEBUG_PRINT("Lap recorded: ");
        // DEBUG_PRINT(state.lap_count);
        // DEBUG_PRINT(", Time: ");
        // DEBUG_PRINT(lap.lap_time_ms);
        // DEBUG_PRINT("ms, Peak: ");
        // DEBUG_PRINTLN(peak_rssi);
  
  // Notify callback if registered
  if (lap_callback) {
    lap_callback(lap);
  }
}

void TimingCore::setupRX5808() {
  Serial.println("Setting up RX5808...");
  
  // Initialize SPI pins
  pinMode(RX5808_DATA_PIN, OUTPUT);
  pinMode(RX5808_CLK_PIN, OUTPUT);
  pinMode(RX5808_SEL_PIN, OUTPUT);
  
  Serial.printf("RX5808 pins - DATA: %d, CLK: %d, SEL: %d\n", 
                RX5808_DATA_PIN, RX5808_CLK_PIN, RX5808_SEL_PIN);
  
  // Set initial states
  digitalWrite(RX5808_SEL_PIN, HIGH);
  digitalWrite(RX5808_CLK_PIN, LOW);
  digitalWrite(RX5808_DATA_PIN, LOW);
  
  delay(100); // Allow module to stabilize
  Serial.println("RX5808 setup complete");
}

void TimingCore::setRX5808Frequency(uint16_t freq_mhz) {
  if (freq_mhz < MIN_FREQ || freq_mhz > MAX_FREQ) {
    Serial.printf("Invalid frequency: %d MHz (valid range: %d-%d)\n", freq_mhz, MIN_FREQ, MAX_FREQ);
    return;
  }
  
  // Convert frequency to register values (from original RotorHazard)
  uint16_t freq_reg = ((freq_mhz - 479) * 16) / 5;
  uint16_t synth_a = 0x8008 | ((freq_reg & 0x0007) << 5) | ((freq_reg & 0x0078) >> 3);
  uint16_t synth_b = 0x8209 | ((freq_reg & 0x0380) << 2);
  
  Serial.printf("Setting frequency to %d MHz (reg=%d, synth_a=0x%04X, synth_b=0x%04X)\n", 
                freq_mhz, freq_reg, synth_a, synth_b);
  
  // Send register values
  sendRX5808Bits(synth_a, 16);
  sendRX5808Bits(synth_b, 16);
  
  state.frequency_mhz = freq_mhz;
  
  // Allow time for frequency to stabilize
  delay(50);
  Serial.printf("Frequency set to %d MHz\n", freq_mhz);
}

void TimingCore::sendRX5808Bits(uint16_t data, uint8_t bit_count) {
  digitalWrite(RX5808_SEL_PIN, LOW);
  delayMicroseconds(1);
  
  // Send bits MSB first
  for (int i = bit_count - 1; i >= 0; i--) {
    digitalWrite(RX5808_CLK_PIN, LOW);
    delayMicroseconds(1);
    
    digitalWrite(RX5808_DATA_PIN, (data >> i) & 1);
    delayMicroseconds(1);
    
    digitalWrite(RX5808_CLK_PIN, HIGH);
    delayMicroseconds(1);
  }
  
  digitalWrite(RX5808_CLK_PIN, LOW);
  digitalWrite(RX5808_SEL_PIN, HIGH);
  delayMicroseconds(1);
}

// Public interface methods (thread-safe for ESP32-C3)
void TimingCore::setFrequency(uint16_t freq_mhz) {
  if (xSemaphoreTake(timing_mutex, portMAX_DELAY)) {
    setRX5808Frequency(freq_mhz);
    xSemaphoreGive(timing_mutex);
  }
}

void TimingCore::setThreshold(uint8_t threshold) {
  if (xSemaphoreTake(timing_mutex, portMAX_DELAY)) {
    state.threshold = threshold;
    // Threshold set
    xSemaphoreGive(timing_mutex);
  }
}

void TimingCore::setActivated(bool active) {
  if (xSemaphoreTake(timing_mutex, portMAX_DELAY)) {
    state.activated = active;
    // Timing activated/deactivated
    xSemaphoreGive(timing_mutex);
  }
}

void TimingCore::reset() {
  if (xSemaphoreTake(timing_mutex, portMAX_DELAY)) {
    state.lap_count = 0;
    state.last_lap_time = 0;
    state.peak_rssi = 0;
    state.crossing_active = false;
    
    // Clear lap buffer
    memset(lap_buffer, 0, sizeof(lap_buffer));
    lap_write_index = 0;
    lap_read_index = 0;
    
    // Timing reset
    xSemaphoreGive(timing_mutex);
  }
}

bool TimingCore::hasNewLap() {
  return lap_read_index != lap_write_index;
}

LapData TimingCore::getNextLap() {
  if (!hasNewLap()) {
    LapData empty = {0};
    return empty;
  }
  
  LapData lap = lap_buffer[lap_read_index];
  lap_read_index = (lap_read_index + 1) % MAX_LAPS_STORED;
  return lap;
}

LapData TimingCore::getLastLap() {
  if (state.lap_count == 0) {
    LapData empty = {0};
    return empty;
  }
  
  uint8_t last_index = (lap_write_index - 1 + MAX_LAPS_STORED) % MAX_LAPS_STORED;
  return lap_buffer[last_index];
}

uint8_t TimingCore::getAvailableLaps() {
  return (lap_write_index - lap_read_index + MAX_LAPS_STORED) % MAX_LAPS_STORED;
}

// Thread-safe state access methods
TimingState TimingCore::getState() const {
  TimingState current_state;
  if (xSemaphoreTake(timing_mutex, pdMS_TO_TICKS(10))) {
    current_state = state;
    xSemaphoreGive(timing_mutex);
  } else {
    // If we can't get the mutex quickly, return a zeroed state
    memset(&current_state, 0, sizeof(current_state));
  }
  return current_state;
}

uint8_t TimingCore::getCurrentRSSI() const {
  uint8_t rssi = 0;
  if (xSemaphoreTake(timing_mutex, pdMS_TO_TICKS(10))) {
    rssi = state.current_rssi;
    xSemaphoreGive(timing_mutex);
  }
  return rssi;
}

uint8_t TimingCore::getPeakRSSI() const {
  uint8_t peak = 0;
  if (xSemaphoreTake(timing_mutex, pdMS_TO_TICKS(10))) {
    peak = state.peak_rssi;
    xSemaphoreGive(timing_mutex);
  }
  return peak;
}

uint16_t TimingCore::getLapCount() const {
  uint16_t count = 0;
  if (xSemaphoreTake(timing_mutex, pdMS_TO_TICKS(10))) {
    count = state.lap_count;
    xSemaphoreGive(timing_mutex);
  }
  return count;
}

bool TimingCore::isActivated() const {
  bool activated = false;
  if (xSemaphoreTake(timing_mutex, pdMS_TO_TICKS(10))) {
    activated = state.activated;
    xSemaphoreGive(timing_mutex);
  }
  return activated;
}

bool TimingCore::isCrossing() const {
  bool crossing = false;
  if (xSemaphoreTake(timing_mutex, pdMS_TO_TICKS(10))) {
    crossing = state.crossing_active;
    xSemaphoreGive(timing_mutex);
  }
  return crossing;
}
