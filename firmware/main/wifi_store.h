#pragma once

// Saved Wi-Fi networks: up to WIFI_STORE_MAX, most recently joined first, kept in NVS
// (namespace "hub", blob "h_nets"). No Wi-Fi calls here, so the simulator uses it too.
// Not thread-safe: net.c serializes every call.

#include <stdbool.h>

#include "net.h"

#define WIFI_STORE_MAX 5

typedef struct {
    char ssid[33];
    char pass[65];
} wifi_net_t;

// Loads the list. `seed_ssid`/`seed_pass` (hubcfg's single network from older firmware or a
// local build) is added when there is no list yet, or when `seed_fresh` (a local build just
// stored new settings).
void wifi_store_init(const char *seed_ssid, const char *seed_pass, bool seed_fresh);
int wifi_store_count(void);
// Entry `i` (0 = most recent). False when out of range.
bool wifi_store_get(int i, wifi_net_t *out);
// The saved entry for `ssid`. False when it isn't saved.
bool wifi_store_find(const char *ssid, wifi_net_t *out);
// Saves (or moves) a network to the front; the oldest drops off a full list. Add and forget
// change the list in memory: wifi_store_flush writes it to NVS.
bool wifi_store_add(const char *ssid, const char *pass);
bool wifi_store_forget(const char *ssid);
void wifi_store_flush(void);
// Saved networks that are in `aps` (a scan), strongest first: indexes into the store.
// Returns how many were written to `out` (at most WIFI_STORE_MAX).
int wifi_store_rank(const net_ap_t *aps, int n, int out[WIFI_STORE_MAX]);
