#pragma once

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

// One network from a scan.
typedef struct {
    char ssid[33];
    int8_t rssi;
    bool secure;   // needs a password
} net_ap_t;

#define NET_SCAN_MAX 20

typedef enum { NET_JOIN_IDLE, NET_JOIN_BUSY, NET_JOIN_OK, NET_JOIN_FAIL } net_join_t;

// Joins the strongest saved network (wifi_store.h), starts SNTP and advertises
// <hostname>.local over mDNS.
void net_start(void);
bool net_connected(void);
// No saved network: the Wi-Fi screen has to set one up.
bool net_needs_setup(void);
bool net_time_valid(void);
const char *net_ip(void);
// The network the board is on ("" when not connected).
void net_ssid(char *out, size_t n);

// Starts a scan. False when the radio is busy (joining): try again shortly.
bool net_scan_start(void);
bool net_scanning(void);
// The last scan's networks, strongest first, one entry per name. `gen` changes with every scan.
int net_scan_results(net_ap_t *out, int max, uint32_t *gen);

// Joins a network; it is saved once the board has an address. Follow it with net_join_state.
void net_join(const char *ssid, const char *pass);
net_join_t net_join_state(void);
// Joins a saved network with its saved password. False when it isn't saved.
bool net_join_saved(const char *ssid);
// Forgets a saved network (and leaves it when the board is on it).
void net_forget(const char *ssid);
// The saved networks' names, most recently joined first. Returns how many.
int net_saved(char (*out)[33], int max);
