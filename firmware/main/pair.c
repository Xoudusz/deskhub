// Pairing over Wi-Fi: POST /pair?name=<PC name>&code=<4-6 digits>. The board shows
// "Pair with <name>?" with the code and the asking address, Deny / Allow for 60 s; Allow
// answers {"token": ..., "host": ...}. No token is needed to ask, so the app shows the same
// code and the user allows only when both match (another device asking at the same moment
// can't know it). One prompt runs at a time; after a Deny or no answer, requests wait 10 s.
#include "pair.h"

#include <ctype.h>
#include <stdio.h>
#include <string.h>

#include "cJSON.h"
#include "display.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "freertos/FreeRTOS.h"
#include "freertos/task.h"
#include "hubcfg.h"
#include "lwip/sockets.h"
#include "ui.h"

static const char *TAG = "pair";

#define ANSWER_TIMEOUT_MS 60000
#define COOLDOWN_US (10 * 1000000LL)

enum { ANSWER_NONE, ANSWER_ALLOW, ANSWER_DENY };
static volatile int s_answer;
static volatile bool s_busy;
static int64_t s_denied_at;
static char s_name[33];
static char s_code[8];
static char s_ip[48];

// The first tap counts; a second one (Deny then Allow) before the prompt hides is ignored.
// Taps come from the LVGL task only.
void pair_answer(bool allow)
{
    if (s_answer == ANSWER_NONE) s_answer = allow ? ANSWER_ALLOW : ANSWER_DENY;
}

static void send_status(httpd_req_t *req, const char *status, const char *msg)
{
    httpd_resp_set_status(req, status);
    httpd_resp_sendstr(req, msg);
}

static void pair_task(void *arg)
{
    httpd_req_t *req = arg;
    s_answer = ANSWER_NONE;
    display_lock();
    display_wake();
    if (ui_saver_shown()) ui_saver_show(false);
    ui_pair_show(s_name, s_code, s_ip);
    display_unlock();

    int waited = 0;
    while (s_answer == ANSWER_NONE && waited < ANSWER_TIMEOUT_MS) {
        vTaskDelay(pdMS_TO_TICKS(100));
        waited += 100;
    }
    display_lock();
    ui_pair_hide();
    display_unlock();

    if (s_answer == ANSWER_ALLOW && !hubcfg()->token[0]) {
        ESP_LOGW(TAG, "pairing with %s: the board has no token yet", s_name);
        send_status(req, "503 Service Unavailable", "the board has no token yet");
    } else if (s_answer == ANSWER_ALLOW) {
        cJSON *m = cJSON_CreateObject();
        cJSON_AddStringToObject(m, "token", hubcfg()->token);
        cJSON_AddStringToObject(m, "host", hubcfg()->host);
        char *text = cJSON_PrintUnformatted(m);
        cJSON *t = cJSON_GetObjectItemCaseSensitive(m, "token");
        if (cJSON_IsString(t) && t->valuestring) memset(t->valuestring, 0, strlen(t->valuestring));
        cJSON_Delete(m);
        if (text) {
            httpd_resp_set_type(req, "application/json");
            httpd_resp_sendstr(req, text);
            memset(text, 0, strlen(text));
            cJSON_free(text);
            ESP_LOGI(TAG, "paired with %s", s_name);
        } else {
            send_status(req, "500 Internal Server Error", "no memory");
        }
    } else if (s_answer == ANSWER_DENY) {
        s_denied_at = esp_timer_get_time();
        ESP_LOGW(TAG, "pairing with %s denied", s_name);
        send_status(req, "403 Forbidden", "denied on the board");
    } else {
        s_denied_at = esp_timer_get_time();
        ESP_LOGW(TAG, "pairing with %s not answered", s_name);
        send_status(req, "408 Request Timeout", "nobody answered on the board");
    }
    httpd_req_async_handler_complete(req);
    s_busy = false;
    vTaskDelete(NULL);
}

static int hex(char c)
{
    if (c >= '0' && c <= '9') return c - '0';
    c = (char)tolower((unsigned char)c);
    return c >= 'a' && c <= 'f' ? c - 'a' + 10 : -1;
}

// Length of the valid UTF-8 sequence at s (1-4), or 0 when it isn't one.
static int utf8_len(const unsigned char *s, size_t avail)
{
    int len = s[0] < 0x80 ? 1 : (s[0] & 0xE0) == 0xC0 ? 2 : (s[0] & 0xF0) == 0xE0 ? 3 : (s[0] & 0xF8) == 0xF0 ? 4 : 0;
    if (len == 0 || (size_t)len > avail || (len == 2 && s[0] < 0xC2)) return 0;
    for (int i = 1; i < len; i++) {
        if ((s[i] & 0xC0) != 0x80) return 0;
    }
    return len;
}

// The query value URL-decoded, without control characters or broken UTF-8, cut at a
// character boundary; "a PC" when empty.
static void decode_name(const char *in, char *out, size_t n)
{
    unsigned char raw[100];
    size_t r = 0;
    for (const char *p = in; *p && r < sizeof(raw); p++) {
        int c = (unsigned char)*p;
        if (c == '+') {
            c = ' ';
        } else if (c == '%' && hex(p[1]) >= 0 && hex(p[2]) >= 0) {
            c = hex(p[1]) * 16 + hex(p[2]);
            p += 2;
        }
        if (c < 0x20 || c == 0x7f) continue;
        raw[r++] = (unsigned char)c;
    }
    size_t o = 0;
    for (size_t i = 0; i < r;) {
        int len = utf8_len(raw + i, r - i);
        if (len == 0) {   // drop a broken byte
            i++;
            continue;
        }
        if (o + len + 1 > n) break;
        memcpy(out + o, raw + i, len);
        o += len;
        i += len;
    }
    out[o] = 0;
    if (!out[0]) strlcpy(out, "a PC", n);
}

// 4 to 6 digits.
static bool code_ok(const char *c)
{
    size_t n = strlen(c);
    if (n < 4 || n > 6) return false;
    for (size_t i = 0; i < n; i++) {
        if (c[i] < '0' || c[i] > '9') return false;
    }
    return true;
}

// The asking device's address, shown on the prompt.
static void peer_ip(httpd_req_t *req, char *out, size_t n)
{
    struct sockaddr_storage a;
    socklen_t len = sizeof(a);
    strlcpy(out, "?", n);
    if (getpeername(httpd_req_to_sockfd(req), (struct sockaddr *)&a, &len) != 0) return;
    if (a.ss_family == AF_INET) {
        inet_ntop(AF_INET, &((struct sockaddr_in *)&a)->sin_addr, out, n);
#if LWIP_IPV6
    } else if (a.ss_family == AF_INET6) {
        const struct sockaddr_in6 *a6 = (const struct sockaddr_in6 *)&a;
        const uint8_t *b = (const uint8_t *)&a6->sin6_addr;
        static const uint8_t mapped[12] = {0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff};
        if (memcmp(b, mapped, 12) == 0) {
            snprintf(out, n, "%u.%u.%u.%u", b[12], b[13], b[14], b[15]);
        } else {
            inet_ntop(AF_INET6, &a6->sin6_addr, out, n);
        }
#endif
    }
}

static esp_err_t pair_handler(httpd_req_t *req)
{
    // Browsers send Origin on every POST; the desktop app never does. Refusing it keeps web
    // pages (DNS rebinding included) from asking for the token through the user's browser.
    if (httpd_req_get_hdr_value_len(req, "Origin") > 0) {
        send_status(req, "403 Forbidden", "pairing from a web page is not allowed");
        return ESP_OK;
    }
    if (s_busy) return httpd_resp_send_err(req, HTTPD_400_BAD_REQUEST, "a pairing prompt is already open");
    if (s_denied_at && esp_timer_get_time() - s_denied_at < COOLDOWN_US) {
        send_status(req, "429 Too Many Requests", "denied a moment ago; try again shortly");
        return ESP_OK;
    }
    char query[192] = "", raw[100] = "", code[16] = "";
    if (httpd_req_get_url_query_str(req, query, sizeof(query)) == ESP_OK) {
        httpd_query_key_value(query, "name", raw, sizeof(raw));
        httpd_query_key_value(query, "code", code, sizeof(code));
    }
    if (!code_ok(code)) return httpd_resp_send_err(req, HTTPD_400_BAD_REQUEST, "code: 4 to 6 digits");
    decode_name(raw, s_name, sizeof(s_name));
    strlcpy(s_code, code, sizeof(s_code));
    peer_ip(req, s_ip, sizeof(s_ip));
    // The prompt waits up to a minute: a task of its own keeps the server answering.
    httpd_req_t *copy = NULL;
    if (httpd_req_async_handler_begin(req, &copy) != ESP_OK) {
        return httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR, "busy");
    }
    s_busy = true;
    if (xTaskCreate(pair_task, "pair", 4096, copy, 5, NULL) != pdPASS) {
        s_busy = false;
        httpd_resp_send_err(copy, HTTPD_500_INTERNAL_SERVER_ERROR, "no memory");
        httpd_req_async_handler_complete(copy);
    }
    return ESP_OK;
}

void pair_register(httpd_handle_t server)
{
    static const httpd_uri_t pair = {.uri = "/pair", .method = HTTP_POST, .handler = pair_handler};
    httpd_register_uri_handler(server, &pair);
}
