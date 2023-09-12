#ifndef config_h
#define config_h

#include <Arduino.h>
#include "util/rhtypes.h"

// Set to 1-8 for manual selection of Arduino node ID/address
// Set to 0 for automatic selection via hardware pin
// See https://github.com/RotorHazard/RotorHazard/wiki/Specification:-Node-hardware-addressing
#define NODE_NUMBER 1

// ******************************************************************** //

// features flags for value returned by READ_RHFEAT_FLAGS command
#define RHFEAT_IAP_FIRMWARE ((uint16_t)0x0010)    // in-application programming of firmware supported
#define RHFEAT_NONE ((uint16_t)0)

// value returned by READ_RHFEAT_FLAGS command
#define RHFEAT_FLAGS_VALUE RHFEAT_NONE

#define SERIAL_BAUD_RATE 115200
#define MULTI_RHNODE_MAX 1

#define ATOMIC_BLOCK(x)
#define ATOMIC_RESTORESTATE

#define RX5808_DATA_PIN 14             //DATA output line to RX5808 module
#define RX5808_SEL_PIN 10              //SEL output line to RX5808 module
#define RX5808_CLK_PIN 13              //CLK output line to RX5808 module
#define RSSI_INPUT_PIN A0              //RSSI input from RX5808
#define NODE_RESET_PIN 12              //Pin to reset paired Arduino via command for ISP

#define DISABLE_SERIAL_PIN 9  //pull pin low (to GND) to disable serial port
#define HARDWARE_SELECT_PIN_1 2
#define HARDWARE_SELECT_PIN_2 3
#define HARDWARE_SELECT_PIN_3 4
#define LEGACY_HARDWARE_SELECT_PIN_1 4
#define LEGACY_HARDWARE_SELECT_PIN_2 5
#define LEGACY_HARDWARE_SELECT_PIN_3 6
#define LEGACY_HARDWARE_SELECT_PIN_4 7
#define LEGACY_HARDWARE_SELECT_PIN_5 8

#define MODULE_LED_ONSTATE HIGH
#define MODULE_LED_OFFSTATE LOW

#define MODULE_LED_PIN LED_BUILTIN     // status LED on processor module

#endif  // config_h
