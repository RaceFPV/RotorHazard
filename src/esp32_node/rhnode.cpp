#ifndef __TEST__
// RotorHazard FPV Race Timing
// Based on Delta 5 Race Timer by Scott Chin
// SPI driver based on fs_skyrf_58g-main.c Written by Simon Chambers
// I2C functions by Mike Ochtman
//
// MIT License
//
// Copyright (c) 2019 Michael Niggel and other contributors
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

#include "config.h"
#include "RssiNode.h"
#include "commands.h"
#include <Wire.h>
#include "rheeprom.h"

// Note: Configure Arduino NODE_NUMBER value in 'config.h'

// firmware version string (prefix allows text to be located in '.bin' file)
const char *firmwareVersionString = "FIRMWARE_VERSION: 1.1.4";

// build date/time strings
const char *firmwareBuildDateString = "FIRMWARE_BUILDDATE: " __DATE__;
const char *firmwareBuildTimeString = "FIRMWARE_BUILDTIME: " __TIME__;

// node processor type
const char *firmwareProcTypeString = "FIRMWARE_PROCTYPE: Arduino";

// i2c address for node
// Node 1 = 8, Node 2 = 10, Node 3 = 12, Node 4 = 14
// Node 5 = 16, Node 6 = 18, Node 7 = 20, Node 8 = 22
uint8_t i2cAddress = 6 + (NODE_NUMBER * 2);
#define SERIALCOM Serial
#define EEPROM_ADRW_RXFREQ 0       //address for stored RX frequency value
#define EEPROM_ADRW_ENTERAT 2      //address for stored 'enterAtLevel'
#define EEPROM_ADRW_EXITAT 4       //address for stored 'exitAtLevel'
#define EEPROM_ADRW_EXPIRE 6       //address for stored catch history expire duration
#define EEPROM_ADRW_CHECKWORD 8    //address for integrity-check value
#define EEPROM_CHECK_VALUE 0x3526  //EEPROM integrity-check value
#define COMMS_MONITOR_TIME_MS 5000 //I2C communications monitor grace/trigger time

// dummy macro
#define LOG_ERROR(...)

Message serialMessage;

Message i2cMessage;

// Defines for fast ADC reads
#define cbi(sfr, bit) (_SFR_BYTE(sfr) &= ~_BV(bit))
#define sbi(sfr, bit) (_SFR_BYTE(sfr) |= _BV(bit))

void i2cInitialize(bool delayFlag);
void i2cReceive(int byteCount);
bool i2cReadAndValidateIoBuffer(byte expectedSize);
void i2cTransmit();

void setModuleLed(bool onFlag);

#if defined(RPI_SIGNAL_PIN) || defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)
void handleRpiSignalAndShutdownActions(mtime_t curTimeMs);
#endif

#ifdef RPI_SIGNAL_PIN
static volatile bool rpiActiveSignalFlag = false;
static volatile mtime_t rpiLastActiveTimeMs = 0;
#define RPI_INACTIVE_DELAYMS 9000  // if no RPi signal for this long then "inactive"
#define RPI_MISSING_DELAYMS 2000
#define GET_RPI_ACTIVESIG_FLAG() (rpiActiveSignalFlag)
#define GET_RPI_LASTACTIVE_TIMEMS() (rpiLastActiveTimeMs)
// have AUX LED mostly on if RPi status is "active"
#define AUXLED_OUT_ONSTATE (rpiActiveSignalFlag ? LOW : HIGH)
#define AUXLED_OUT_OFFSTATE (rpiActiveSignalFlag ? HIGH : LOW)
#else
#define GET_RPI_ACTIVESIG_FLAG() (false)
#define GET_RPI_LASTACTIVE_TIMEMS() (0)
#define AUXLED_OUT_ONSTATE HIGH
#define AUXLED_OUT_OFFSTATE LOW
#endif

#ifdef AUXLED_OUTPUT_PIN
static volatile bool auxLedOutEnabledFlag = false;
#endif

#ifdef BUZZER_OUTPUT_PIN
void setBuzzerState(bool onFlag);
static volatile int buzzerBeepDurationCounter = 0;
static volatile int lastCommActivityTimeMs = 0;
#endif

#if defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)
static volatile bool shutdownButtonPressedFlag = false;
static volatile bool shutdownHasBeenStartedFlag = false;
static volatile bool rpiSignalMissingFlag = false;
#endif

#if ((!defined(NODE_NUMBER)) || (!NODE_NUMBER))
// Configure the I2C address based on input-pin level.
void configI2cAddress()
{
    // current hardware selection
    pinMode(HARDWARE_SELECT_PIN_1, INPUT_PULLUP);
    pinMode(HARDWARE_SELECT_PIN_2, INPUT_PULLUP);
    pinMode(HARDWARE_SELECT_PIN_3, INPUT_PULLUP);
    // legacy selection - DEPRECATED
    pinMode(LEGACY_HARDWARE_SELECT_PIN_1, INPUT_PULLUP);
    pinMode(LEGACY_HARDWARE_SELECT_PIN_2, INPUT_PULLUP);
    pinMode(LEGACY_HARDWARE_SELECT_PIN_3, INPUT_PULLUP);
    pinMode(LEGACY_HARDWARE_SELECT_PIN_4, INPUT_PULLUP);
    pinMode(LEGACY_HARDWARE_SELECT_PIN_5, INPUT_PULLUP);

    delay(100);  // delay a bit a let pin levels settle before reading inputs

    // check if legacy spec pins are in use (2-5 only)
    if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_2) == LOW ||
        digitalRead(LEGACY_HARDWARE_SELECT_PIN_3) == LOW ||
        digitalRead(LEGACY_HARDWARE_SELECT_PIN_4) == LOW ||
        digitalRead(LEGACY_HARDWARE_SELECT_PIN_5) == LOW)
    {
        // legacy spec
        if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_1) == HIGH)
        {
            if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_2) == LOW)
                i2cAddress = 8;
            else if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_3) == LOW)
                i2cAddress = 10;
            else if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_4) == LOW)
                i2cAddress = 12;
            else if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_5) == LOW)
                i2cAddress = 14;
        }
        else
        {
            if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_2) == LOW)
                i2cAddress = 16;
            else if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_3) == LOW)
                i2cAddress = 18;
            else if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_4) == LOW)
                i2cAddress = 20;
            else if (digitalRead(LEGACY_HARDWARE_SELECT_PIN_5) == LOW)
                i2cAddress = 22;
        }
    }
    else
    {   // use standard selection
        i2cAddress = 0;
        if (digitalRead(HARDWARE_SELECT_PIN_1) == LOW)
            i2cAddress |= 1;
        if (digitalRead(HARDWARE_SELECT_PIN_2) == LOW)
            i2cAddress |= 2;
        if (digitalRead(HARDWARE_SELECT_PIN_3) == LOW)
            i2cAddress |= 4;
        i2cAddress = 8 + (i2cAddress * 2);
    }
}
#endif  // ((!defined(NODE_NUMBER)) || (!NODE_NUMBER))

// Initialize program
void setup()
{
    pinMode(MODULE_LED_PIN, OUTPUT);

#ifdef AUXLED_OUTPUT_PIN
    pinMode(AUXLED_OUTPUT_PIN, OUTPUT);
    digitalWrite(AUXLED_OUTPUT_PIN, AUXLED_OUT_OFFSTATE);
#endif


    RssiNode::multiRssiNodeCount = 1;
    RssiNode *rssiNodePtr = &(RssiNode::rssiNodeArray[0]);
    rssiNodePtr->initRx5808Pins(0);

    // init pin used to reset paired Arduino via RESET_PAIRED_NODE command
    pinMode(NODE_RESET_PIN, INPUT_PULLUP);

    // init pin that can be pulled low (to GND) to disable serial port
    pinMode(DISABLE_SERIAL_PIN, INPUT_PULLUP);

#if (!defined(NODE_NUMBER)) || (!NODE_NUMBER)
    configI2cAddress();
#else
    delay(100);  // delay a bit a let pin level settle before reading input
#endif

    if (digitalRead(DISABLE_SERIAL_PIN) == HIGH)
    {
        Serial.begin(SERIAL_BAUD_RATE);  // Start serial interface
        while (!Serial) {};  // Wait for the Serial port to initialize
    }

    i2cInitialize(false);  // setup I2C address and callbacks

    // set ADC prescaler to 16 to speedup ADC readings
    //// BANDAID
    // sbi(ADCSRA, ADPS2);
    // cbi(ADCSRA, ADPS1);
    // cbi(ADCSRA, ADPS0);

    // if EEPROM-check value matches then read stored values
    if (eepromReadWord(EEPROM_ADRW_CHECKWORD) == EEPROM_CHECK_VALUE)
    {
        rssiNodePtr->setVtxFreq(eepromReadWord(EEPROM_ADRW_RXFREQ));
        rssiNodePtr->setEnterAtLevel(eepromReadWord(EEPROM_ADRW_ENTERAT));
        rssiNodePtr->setExitAtLevel(eepromReadWord(EEPROM_ADRW_EXITAT));
    }
    else
    {    // if no match then initialize EEPROM values
        eepromWriteWord(EEPROM_ADRW_RXFREQ, rssiNodePtr->getVtxFreq());
        eepromWriteWord(EEPROM_ADRW_ENTERAT, rssiNodePtr->getEnterAtLevel());
        eepromWriteWord(EEPROM_ADRW_EXITAT, rssiNodePtr->getExitAtLevel());
        eepromWriteWord(EEPROM_ADRW_CHECKWORD, EEPROM_CHECK_VALUE);
    }

    rssiNodePtr->initRxModule();  //init and set RX5808 to default frequency
    rssiNodePtr->rssiInit();      //initialize RSSI processing
}

static bool commsMonitorEnabledFlag = false;
static mtime_t commsMonitorLastResetTime = 0;

// Main loop
void loop()
{
    static mtime_t loopMillis = 0;
#ifdef BUZZER_OUTPUT_PIN
    static bool waitingForFirstCommsFlag = true;
#endif

    mtime_t curTimeMs = millis();
    if (curTimeMs > loopMillis)
    {  // limit to once per millisecond

        // read raw RSSI close to taking timestamp
        bool crossingFlag;
        if (RssiNode::multiRssiNodeCount <= (uint8_t)1)
            crossingFlag = RssiNode::rssiNodeArray[0].rssiProcess(curTimeMs);
        else
        {
            crossingFlag = false;
            for (uint8_t nIdx=0; nIdx<RssiNode::multiRssiNodeCount; ++nIdx)
            {
                RssiNode::rssiNodeArray[nIdx].rssiProcess(curTimeMs);
                curTimeMs = millis();
            }
        }

        // update settings and status LED

        RssiNode *rssiNodePtr = getCmdRssiNodePtr();

        uint8_t changeFlags;
        ATOMIC_BLOCK(ATOMIC_RESTORESTATE)
        {
            changeFlags = settingChangedFlags;
            settingChangedFlags &= COMM_ACTIVITY;  // clear all except COMM_ACTIVITY
        }

        bool oldActFlag = rssiNodePtr->getActivatedFlag();

                      // set freq here if Arduino running single RX5808 module
                      //  otherwise set in 'commands'
        if (changeFlags & FREQ_SET)
        {
            uint16_t newVtxFreq;
            ATOMIC_BLOCK(ATOMIC_RESTORESTATE)
            {
                newVtxFreq = rssiNodePtr->getVtxFreq();
            }
            rssiNodePtr->setRxModuleToFreq(newVtxFreq);
            rssiNodePtr->setActivatedFlag(true);

            if (changeFlags & FREQ_CHANGED)
            {
                eepromWriteWord(EEPROM_ADRW_RXFREQ, newVtxFreq);
                rssiNodePtr->rssiStateReset();  // restart rssi peak tracking for node
            }
        }

        // also allow READ_LAP_STATS command to activate operations
        //  so they will resume after node or I2C bus reset
        if (!rssiNodePtr->getActivatedFlag() && (changeFlags & LAPSTATS_READ))
            rssiNodePtr->setActivatedFlag(true);

        if (commsMonitorEnabledFlag)
        {
            if (changeFlags & COMM_ACTIVITY)
            {  //communications activity detected; update comms monitor time
                commsMonitorLastResetTime = curTimeMs;
            }
            else if (curTimeMs - commsMonitorLastResetTime > COMMS_MONITOR_TIME_MS)
            {  //too long since last communications activity detected
                commsMonitorEnabledFlag = false;
                // redo init, which should release I2C pins (SDA & SCL) if "stuck"
                i2cInitialize(true);
            }
        }
        else if (oldActFlag && (changeFlags & LAPSTATS_READ) &&
                (changeFlags & SERIAL_CMD_MSG) == (uint8_t)0)
        {  //if activated and I2C LAPSTATS_READ cmd received then enable comms monitor
            commsMonitorEnabledFlag = true;
            commsMonitorLastResetTime = curTimeMs;
        }

        if (changeFlags & ENTERAT_CHANGED)
            eepromWriteWord(EEPROM_ADRW_ENTERAT, rssiNodePtr->getEnterAtLevel());
        if (changeFlags & EXITAT_CHANGED)
            eepromWriteWord(EEPROM_ADRW_EXITAT, rssiNodePtr->getExitAtLevel());

        // Status LED
        if (curTimeMs <= 1000)
        {  //flash two times during first second of running
            if (curTimeMs >= 500)  //don't check until 500ms elapsed
            {
                const int ti = (int)(curTimeMs-500) / 100;
                const bool sFlag = (ti == 1 || ti == 3);
                setModuleLed(sFlag);
#ifdef BUZZER_OUTPUT_PIN
                setBuzzerState(sFlag);
#endif
            }
        }
        else if ((int)(curTimeMs % 20) == 0)
        {  //only run every 20ms (so flashes/beeps last longer and less CPU load)

#ifdef BUZZER_OUTPUT_PIN
            if (buzzerBeepDurationCounter > 0)
            {
                if (--buzzerBeepDurationCounter <= 0)
                    setBuzzerState(false);
            }
#endif
            // if crossing or communications activity then LED on
            if (crossingFlag)
                setModuleLed(true);
            else if (changeFlags & COMM_ACTIVITY)
            {
                setModuleLed(true);
                settingChangedFlags = 0;  // clear COMM_ACTIVITY flag
#ifdef BUZZER_OUTPUT_PIN
                lastCommActivityTimeMs = curTimeMs;
                if (waitingForFirstCommsFlag && (changeFlags & LAPSTATS_READ))
                {
                    waitingForFirstCommsFlag = false;
                    setBuzzerState(true);  // beep when operations activated
                    buzzerBeepDurationCounter = 1;
                }
#endif
            }
            else
                setModuleLed(curTimeMs % 2000 == 0);  // blink

#if defined(RPI_SIGNAL_PIN) || defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)
            handleRpiSignalAndShutdownActions(curTimeMs);
#endif
        }

#ifdef AUXLED_OUTPUT_PIN  // show fast blink while shutdown button pressed
        if (shutdownButtonPressedFlag && (!shutdownHasBeenStartedFlag) && (!rpiSignalMissingFlag))
            digitalWrite(AUXLED_OUTPUT_PIN, ((int)((curTimeMs/2) % 40) == 0) ? HIGH : LOW);
#endif

        loopMillis = curTimeMs;
    }
}


void i2cInitialize(bool delayFlag)
{
    setModuleLed(true);
    Wire.end();  // release I2C pins (SDA & SCL), in case they are "stuck"
    if (delayFlag)   // do delay if called via comms monitor
        delay(250);  //  to help bus reset and show longer LED flash
    setModuleLed(false);

    Wire.begin(i2cAddress);  // I2C address setup
    Wire.onReceive(i2cReceive);   // Trigger 'i2cReceive' function on incoming data
    Wire.onRequest(i2cTransmit);  // Trigger 'i2cTransmit' function for outgoing data, on master request

    // BANDAID
    //TWAR = (i2cAddress << 1) | 1;  // enable broadcasts to be received
}

// Function called by twi interrupt service when master sends information to the node
// or when master sets up a specific read request
void i2cReceive(int byteCount)
{  // Number of bytes in rx buffer
   // If byteCount is zero, the master only checked for presence of the node device, no response necessary
    if (byteCount == 0)
    {
        LOG_ERROR("no bytes to receive?");
        return;
    }

    if (byteCount != Wire.available())
    {
        LOG_ERROR("rx byte count and wire available don't agree");
    }

    i2cMessage.command = Wire.read();  // The first byte sent is a command byte

    if (i2cMessage.command > 0x50)
    {  // Commands > 0x50 are writes TO this node
        byte expectedSize = i2cMessage.getPayloadSize();
        if (expectedSize > 0 && i2cReadAndValidateIoBuffer(expectedSize))
        {
            i2cMessage.handleWriteCommand(false);
        }
        i2cMessage.buffer.size = 0;
    }
    else
    {  // Otherwise this is a request FROM this device
        if (Wire.available())
        {  // There shouldn't be any data present on the line for a read request
            LOG_ERROR("Wire.available() on a read request.", ioCommand, HEX);
            while (Wire.available())
            {
                Wire.read();
            }
        }
    }
}

bool i2cReadAndValidateIoBuffer(byte expectedSize)
{
    uint8_t checksum;

    for (i2cMessage.buffer.size = 0; i2cMessage.buffer.size < expectedSize + 1;
            i2cMessage.buffer.size++)
    {
        if (!Wire.available())
        {
            return false;
        }
        i2cMessage.buffer.data[i2cMessage.buffer.size] = Wire.read();
    }

    checksum = i2cMessage.buffer.calculateChecksum(expectedSize);

    if (i2cMessage.buffer.data[i2cMessage.buffer.size-1] == checksum)
    {
        return true;
    }
    else
    {
        LOG_ERROR("Invalid checksum", checksum);
        return false;
    }
}

// Function called by twi interrupt service when the Master wants to get data from the node
// No parameters and no returns
// A transmit buffer (ioBuffer) is populated with the data before sending.
void i2cTransmit()
{
    i2cMessage.handleReadCommand(false);

    if (i2cMessage.buffer.size > 0)
    {  // If there is pending data, send it
        Wire.write((byte *)i2cMessage.buffer.data, i2cMessage.buffer.size);
        i2cMessage.buffer.size = 0;
    }
}

#endif

void serialEvent()
{
    while (SERIALCOM.available())
    {
        uint8_t nextByte = SERIALCOM.read();
        if (serialMessage.buffer.size == 0)
        {
            // new command
            serialMessage.command = nextByte;
            if (serialMessage.command > 0x50)
            {  // Commands > 0x50 are writes TO this node
                byte expectedSize = serialMessage.getPayloadSize();
                if (expectedSize > 0)
                {
                    serialMessage.buffer.index = 0;
                    serialMessage.buffer.size = expectedSize + 1;  // include checksum byte
                }
            }
            else
            {
                serialMessage.handleReadCommand(true);

                if (serialMessage.buffer.size > 0)
                {  // If there is pending data, send it
                    SERIALCOM.write((byte *)serialMessage.buffer.data, serialMessage.buffer.size);
                    serialMessage.buffer.size = 0;
                }
            }
        }
        else
        {
            // existing command
            serialMessage.buffer.data[serialMessage.buffer.index++] = nextByte;
            if (serialMessage.buffer.index == serialMessage.buffer.size)
            {
                uint8_t checksum = serialMessage.buffer.calculateChecksum(
                                           serialMessage.buffer.size - 1);
                if (serialMessage.buffer.data[serialMessage.buffer.size - 1] == checksum)
                {
                    serialMessage.handleWriteCommand(true);
                }
                else
                {
                    LOG_ERROR("Invalid checksum", checksum);
                }
                serialMessage.buffer.size = 0;
            }
        }
    }
}

void setModuleLed(bool onFlag)
{
    static bool currentStatusLedFlag = false;

    if (onFlag)
    {
        if (!currentStatusLedFlag)
        {
            currentStatusLedFlag = true;
            digitalWrite(MODULE_LED_PIN, MODULE_LED_ONSTATE);
#ifdef AUXLED_OUTPUT_PIN
            if (auxLedOutEnabledFlag)
                digitalWrite(AUXLED_OUTPUT_PIN, AUXLED_OUT_ONSTATE);
#endif
        }
    }
    else
    {
        if (currentStatusLedFlag)
        {
            currentStatusLedFlag = false;
            digitalWrite(MODULE_LED_PIN, MODULE_LED_OFFSTATE);
#ifdef AUXLED_OUTPUT_PIN
            if (auxLedOutEnabledFlag)
                digitalWrite(AUXLED_OUTPUT_PIN, AUXLED_OUT_OFFSTATE);
#endif
        }
    }
}

#ifdef BUZZER_OUTPUT_PIN
void setBuzzerState(bool onFlag)
{
    static bool currentBuzzerStateFlag = false;

    if (onFlag)
    {
        if (!currentBuzzerStateFlag)
        {
            currentBuzzerStateFlag = true;
            pinMode(BUZZER_OUTPUT_PIN, OUTPUT);
#ifdef NUCLEARHAZARD_HARDWARE
            analogWrite(BUZZER_OUTPUT_PIN, 512);
#else
            digitalWrite(BUZZER_OUTPUT_PIN, BUZZER_OUT_ONSTATE);
#endif
        }
    }
    else
    {
        if (currentBuzzerStateFlag)
        {
            currentBuzzerStateFlag = false;
            digitalWrite(BUZZER_OUTPUT_PIN, BUZZER_OUT_OFFSTATE);
            pinMode(BUZZER_OUTPUT_PIN, INPUT);
        }
    }
}
#endif

#if defined(RPI_SIGNAL_PIN) || defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)

void handleRpiSignalAndShutdownActions(mtime_t curTimeMs)
{
#if defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)
    static bool prevSdButtonFlag = false;
    static bool prevSdStartedFlag = false;
#endif

#ifdef RPI_SIGNAL_PIN
            const int rpiSigVal = digitalRead(RPI_SIGNAL_PIN);
            if (rpiActiveSignalFlag)
            {  //RPi is currently "active"
                if (rpiSigVal == RPI_SIGNAL_ONSTATE)
                {  //new RPI status/heartbeat signal detected
                    rpiLastActiveTimeMs = curTimeMs;
                }
                else if (curTimeMs - rpiLastActiveTimeMs > RPI_INACTIVE_DELAYMS)
                {  //enough time has elapsed to declare RPi "inactive" (shutdown)
                    rpiActiveSignalFlag = false;
                }
#ifdef BUZZER_OUTPUT_PIN
                else if ((!shutdownHasBeenStartedFlag) && (!rpiSignalMissingFlag) &&
                         rpiLastActiveTimeMs > 0 &&
                         curTimeMs - rpiLastActiveTimeMs > RPI_MISSING_DELAYMS &&
                         curTimeMs - lastCommActivityTimeMs > RPI_INACTIVE_DELAYMS*10)
                {  //RPi heartbeat stopped and no recent comms (so not system-reset via server)
                    rpiActiveSignalFlag = false;
                    rpiSignalMissingFlag = prevSdStartedFlag = true;  // signal shutdown in progress
                }
#endif
            }
            else if (rpiSigVal == RPI_SIGNAL_ONSTATE)
            {  //RPi is going from "inactive" to "active"
                rpiActiveSignalFlag = true;
                rpiLastActiveTimeMs = curTimeMs;
#ifdef BUZZER_OUTPUT_PIN
                if (rpiSignalMissingFlag)
                {  //RPi previously detected as missing; indicate no longer missing
                    rpiSignalMissingFlag = false;
                    if (!shutdownHasBeenStartedFlag)  // if shutdown not really in progress then
                        prevSdStartedFlag = false;    // clear tracking flag
                }
#endif
#ifdef AUXLED_OUTPUT_PIN
                auxLedOutEnabledFlag = true;  // enable AUX LED
#endif
                setModuleLed(true);   // turn AUX LED on right away
                setModuleLed(false);
            }
#endif  // RPI_SIGNAL_PIN

#if defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)
            if ((!shutdownHasBeenStartedFlag) && (!rpiSignalMissingFlag))
            {
                if (GET_RPI_ACTIVESIG_FLAG() || GET_RPI_LASTACTIVE_TIMEMS() == 0)
                {  //RPi state is "active" (or no active signal seen at all)
                    if (shutdownButtonPressedFlag)
                    {
                        if (!prevSdButtonFlag)
                        {  //shutdown button was just pressed
#ifdef BUZZER_OUTPUT_PIN  // do short beep when shutdown button pressed
                            setBuzzerState(true);
                            buzzerBeepDurationCounter = 1;
#endif
#ifdef AUXLED_OUTPUT_PIN
                            auxLedOutEnabledFlag = false;  // don't update AUX LED elsewhere
#endif
                        }
                    }
                    else if (prevSdButtonFlag)
                    {  //shutdown button released before shutdown started
#ifdef AUXLED_OUTPUT_PIN
                        if (GET_RPI_LASTACTIVE_TIMEMS() > 0)
                            auxLedOutEnabledFlag = true;  // resume AUX LED updates
                        digitalWrite(AUXLED_OUTPUT_PIN, AUXLED_OUT_OFFSTATE);
#endif
                    }
                }
                else if (shutdownButtonPressedFlag)
                {  //RPi went inactive while button pressed; treat as shutdown
                    shutdownHasBeenStartedFlag = true;
                    shutdownButtonPressedFlag = prevSdButtonFlag = false;
                }
            }
            else
            {  //shutdown has been started
                if (!prevSdStartedFlag)
                {  //shutdown just started
#ifdef AUXLED_OUTPUT_PIN  // show steady AUX LED
                    auxLedOutEnabledFlag = false;  // disable AUX LED updates
                    digitalWrite(AUXLED_OUTPUT_PIN, AUXLED_OUT_OFFSTATE);
#endif
#ifdef BUZZER_OUTPUT_PIN  // play semi-long beep
                    prevSdStartedFlag = true;
                    buzzerBeepDurationCounter = 1010;
                    setBuzzerState(true);
#else
                    shutdownHasBeenStartedFlag = rpiSignalMissingFlag = false;
#endif
                }
#ifdef BUZZER_OUTPUT_PIN
                else
                {  //shutdown is in progress
                    if (buzzerBeepDurationCounter > 500)
                    {
                        if (buzzerBeepDurationCounter <= 1000)
                        {  //stop playing long beep
                            buzzerBeepDurationCounter = 0;
                            setBuzzerState(false);
                        }
                    }
                    else if (GET_RPI_ACTIVESIG_FLAG())
                    {
                        if ((int)(curTimeMs % 1000) == 0)
                        {  //play periodic short beeps until shutdown is complete
                            buzzerBeepDurationCounter = 1;
                            setBuzzerState(true);
                        }
                    }
                    else if (GET_RPI_LASTACTIVE_TIMEMS() > 0)
                    {  //shutdown has completed; reset flags in case system resumes
                        prevSdStartedFlag = shutdownHasBeenStartedFlag =
                                                   rpiSignalMissingFlag = false;
                        prevSdButtonFlag = shutdownButtonPressedFlag = false;
#ifdef AUXLED_OUTPUT_PIN  // turn off AUX LED
                        auxLedOutEnabledFlag = false;  // disable AUX LED updates
                        digitalWrite(AUXLED_OUTPUT_PIN, AUXLED_OUT_OFFSTATE);
#endif
                        buzzerBeepDurationCounter = 30;  // play final long beep
                        setBuzzerState(true);
                    }
                }
#endif  // BUZZER_OUTPUT_PIN
            }
            prevSdButtonFlag = shutdownButtonPressedFlag;

#endif  // defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)
}
#endif  // defined(RPI_SIGNAL_PIN) || defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)

// Handle status message sent from server
void handleStatusMessage(byte msgTypeVal, byte msgDataVal)
{
#if defined(AUXLED_OUTPUT_PIN) || defined(BUZZER_OUTPUT_PIN)
    switch (msgTypeVal)
    {
        case STATMSG_SDBUTTON_STATE:
            shutdownButtonPressedFlag = (msgDataVal != (byte)0);
            break;

        case STATMSG_SHUTDOWN_STARTED:
            shutdownButtonPressedFlag = false;
            shutdownHasBeenStartedFlag = true;
            break;
    }
#endif
}