#pragma once

#include <stdbool.h>
#include "esp_http_server.h"

// POST /pair?name=<PC name> (no token): the board asks "Pair with <name>?" on screen and,
// when someone taps Allow, answers with its token so the app never has to type it.
void pair_register(httpd_handle_t server);
// The on-screen prompt's answer.
void pair_answer(bool allow);
