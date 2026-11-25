#pragma once

// Setup over the USB cable: the desktop app sends lines "@deskhub {json}" on the board's
// console port and reads replies of the same shape among the log lines. Commands:
//   {"type":"info"}                      -> version, variant, host, token, ssid, ip, saved
//   {"type":"scan"}                      -> networks the board sees
//   {"type":"wifi","ssid":..,"pass":..}  -> joins and saves the network: ok + ip, or error
// The cable means someone has the board in hand (they could flash it anyway), so the token
// is given out here without a prompt.
void usb_setup_start(void);
