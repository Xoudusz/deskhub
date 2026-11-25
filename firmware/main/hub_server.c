#include "hub_server.h"

#include <stdlib.h>
#include <math.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

#include "board.h"
#include "cJSON.h"
#include "esp_app_desc.h"
#include "display.h"
#include "esp_heap_caps.h"
#include "esp_http_server.h"
#include "esp_system.h"
#include "esp_log.h"
#include "esp_timer.h"
#include "hub_dispatch.h"
#include "hub_model.h"
#include "hubcfg.h"
#include "net.h"
#include "ota.h"
#include "pair.h"
#include "sdkconfig.h"
#include "ui.h"

static const char *TAG = "server";

#define MAX_FRAME (300 * 1024)
#define ONLINE_TIMEOUT_MS 5000
#define PREAUTH_MAX 256

static httpd_handle_t s_server;
static volatile int s_client = -1;
static volatile uint32_t s_last_rx_ms; // 32-bit so cross-core reads are atomic

static uint32_t now_ms(void)
{
    return (uint32_t)(esp_timer_get_time() / 1000);
}

static void copy_str(char *dst, size_t n, const cJSON *obj, const char *key)
{
    const cJSON *v = cJSON_GetObjectItemCaseSensitive(obj, key);
    if (cJSON_IsString(v) && v->valuestring) {
        strlcpy(dst, v->valuestring, n);
    } else {
        dst[0] = 0;
    }
}


// {"type":"hello_ok","version","variant","creds","location":{city,lat,lon,tz,fahrenheit,h24}}:
// the app picks release images by variant, offers them once creds are "nvs", and starts its
// Location card from the board's values.
static void send_hello_ok(void)
{
    const hub_cfg_t *c = hubcfg();
    cJSON *m = cJSON_CreateObject();
    cJSON_AddStringToObject(m, "type", "hello_ok");
    cJSON_AddStringToObject(m, "version", esp_app_get_description()->version);
    cJSON_AddStringToObject(m, "variant", hubcfg_variant());
    cJSON_AddStringToObject(m, "creds", hubcfg_creds());
    cJSON *loc = cJSON_AddObjectToObject(m, "location");
    cJSON_AddStringToObject(loc, "city", c->city);
    cJSON_AddStringToObject(loc, "lat", c->lat);
    cJSON_AddStringToObject(loc, "lon", c->lon);
    cJSON_AddStringToObject(loc, "tz", c->tz);
    cJSON_AddBoolToObject(loc, "fahrenheit", c->fahrenheit);
    cJSON_AddBoolToObject(loc, "h24", c->h24);
    char *text = cJSON_PrintUnformatted(m);
    if (text) {
        hub_server_send(text);
        cJSON_free(text);
    }
    cJSON_Delete(m);
}

static void on_text(int fd, char *text)
{
    cJSON *m = cJSON_Parse(text);
    if (!m) {
        ESP_LOGW(TAG, "bad json");
        return;
    }
    char type[24];
    copy_str(type, sizeof(type), m, "type");

    if (strcmp(type, "hello") == 0) {
        char token[64];
        copy_str(token, sizeof(token), m, "token");
        if (hubcfg_token_ok(token)) {
            if (s_client >= 0 && s_client != fd) {
                httpd_sess_trigger_close(s_server, s_client);
            }
            s_client = fd;
            s_last_rx_ms = now_ms();
            ESP_LOGI(TAG, "companion connected (fd %d)", fd);
            send_hello_ok();
        } else {
            ESP_LOGW(TAG, "wrong token from fd %d", fd);
            httpd_sess_trigger_close(s_server, fd);
        }
    } else if (fd != s_client) {
        // Ignore everything from sockets that have not said hello.
    } else {
        s_last_rx_ms = now_ms();
        hub_dispatch_json(m, type);
    }
    cJSON_Delete(m);
}

// ---- http / ws plumbing ----------------------------------------------------

static esp_err_t ws_handler(httpd_req_t *req)
{
    if (req->method == HTTP_GET) {
        return ESP_OK; // handshake done
    }
    httpd_ws_frame_t frame = {0};
    esp_err_t err = httpd_ws_recv_frame(req, &frame, 0);
    if (err != ESP_OK) return err;
    if (frame.len == 0 || frame.len > MAX_FRAME) return frame.len ? ESP_FAIL : ESP_OK;
    // Until a socket says hello, only accept a frame big enough for the hello itself.
    if (httpd_req_to_sockfd(req) != s_client && frame.len > PREAUTH_MAX) return ESP_FAIL;

    uint8_t *buf = heap_caps_malloc(frame.len + 1, MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT);
    if (!buf) return ESP_ERR_NO_MEM;
    frame.payload = buf;
    err = httpd_ws_recv_frame(req, &frame, frame.len);
    if (err == ESP_OK) {
        buf[frame.len] = 0;
        int fd = httpd_req_to_sockfd(req);
        if (frame.type == HTTPD_WS_TYPE_TEXT) {
            on_text(fd, (char *)buf);
        } else if (frame.type == HTTPD_WS_TYPE_BINARY && fd == s_client) {
            if (hub_dispatch_binary(buf, frame.len)) s_last_rx_ms = now_ms();
        }
    }
    free(buf);
    return err;
}

static esp_err_t root_handler(httpd_req_t *req)
{
    char text[96];
    snprintf(text, sizeof(text), "Desk Hub %s: %s\n", esp_app_get_description()->version,
             hub_server_pc_online() ? "companion online" : "waiting for companion");
    httpd_resp_set_type(req, "text/plain");
    return httpd_resp_sendstr(req, text);
}

// ---- debug endpoints: /screenshot and /tap, both need ?token= --------------

static bool token_ok(httpd_req_t *req, char *query, size_t qlen)
{
    char token[64];
    if (httpd_req_get_url_query_str(req, query, qlen) != ESP_OK ||
        httpd_query_key_value(query, "token", token, sizeof(token)) != ESP_OK ||
        !hubcfg_token_ok(token)) {
        httpd_resp_send_err(req, HTTPD_403_FORBIDDEN, "bad token");
        return false;
    }
    return true;
}

static void put32(uint8_t *p, uint32_t v)
{
    p[0] = v; p[1] = v >> 8; p[2] = v >> 16; p[3] = v >> 24;
}

// Returns the current frame as a 24-bit BMP.
static esp_err_t screenshot_handler(httpd_req_t *req)
{
    char query[128];
    if (!token_ok(req, query, sizeof(query))) return ESP_OK;

    const int W = BOARD_WIDTH, H = BOARD_HEIGHT;
    size_t fb_bytes = (size_t)W * H * 2;
    uint16_t *copy = heap_caps_malloc(fb_bytes, MALLOC_CAP_SPIRAM);
    if (!copy) return httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR, "no memory");
    // Copy under the LVGL lock so the frame isn't half-rendered.
    display_lock();
    bool ok = board_snapshot(copy);
    display_unlock();
    if (!ok) {
        free(copy);
        return httpd_resp_send_err(req, HTTPD_500_INTERNAL_SERVER_ERROR, "no frame buffer");
    }

    uint8_t hdr[54] = {'B', 'M'};
    uint32_t row = W * 3, img = row * H;
    put32(hdr + 2, 54 + img);
    put32(hdr + 10, 54);
    put32(hdr + 14, 40);
    put32(hdr + 18, W);
    put32(hdr + 22, H);
    hdr[26] = 1;
    hdr[28] = 24;
    put32(hdr + 34, img);
    httpd_resp_set_type(req, "image/bmp");
    httpd_resp_send_chunk(req, (const char *)hdr, sizeof(hdr));

    enum { ROWS = 16 };
    uint8_t *buf = malloc(row * ROWS);
    esp_err_t err = buf ? ESP_OK : ESP_ERR_NO_MEM;
    // BMP rows go bottom-up, pixels as B, G, R.
    for (int y = H - 1; y >= 0 && err == ESP_OK; y -= ROWS) {
        int n = 0;
        for (int r = 0; r < ROWS && y - r >= 0; r++, n++) {
            const uint16_t *src = copy + (size_t)(y - r) * W;
            uint8_t *dst = buf + (size_t)r * row;
            for (int x = 0; x < W; x++) {
                uint16_t p = src[x];
                dst[3 * x + 0] = (p & 0x1f) << 3;
                dst[3 * x + 1] = ((p >> 5) & 0x3f) << 2;
                dst[3 * x + 2] = (p >> 11) << 3;
            }
        }
        err = httpd_resp_send_chunk(req, (const char *)buf, row * n);
    }
    free(buf);
    free(copy);
    if (err == ESP_OK) httpd_resp_send_chunk(req, NULL, 0);
    return err;
}

static esp_err_t tap_handler(httpd_req_t *req)
{
    char query[128], v[8];
    if (!token_ok(req, query, sizeof(query))) return ESP_OK;
    int x = -1, y = -1;
    if (httpd_query_key_value(query, "x", v, sizeof(v)) == ESP_OK) x = atoi(v);
    if (httpd_query_key_value(query, "y", v, sizeof(v)) == ESP_OK) y = atoi(v);
    int x2 = x;
    if (httpd_query_key_value(query, "x2", v, sizeof(v)) == ESP_OK) x2 = atoi(v);
    if (x < 0 || y < 0 || x >= BOARD_WIDTH || y >= BOARD_HEIGHT) {
        return httpd_resp_send_err(req, HTTPD_400_BAD_REQUEST, "x/y out of range");
    }
    if (x2 < 0 || x2 >= BOARD_WIDTH) x2 = x;
    if (x2 == x) display_inject_tap(x, y);
    else display_inject_drag(x, y, x2, 250);
    return httpd_resp_sendstr(req, "ok\n");
}

// Rendering performance. ?full=1 also times one forced full-screen redraw.
static esp_err_t perf_handler(httpd_req_t *req)
{
    char query[128], v[4];
    if (!token_ok(req, query, sizeof(query))) return ESP_OK;
    display_perf_t p = display_perf();
    long full_us = -1;
    if (httpd_query_key_value(query, "full", v, sizeof(v)) == ESP_OK && v[0] == '1') {
        display_lock();
        full_us = (long)display_time_full_redraw();
        display_unlock();
    }
    char buf[200];
    snprintf(buf, sizeof(buf),
             "{\"fps\":%lu,\"avg_ms\":%.1f,\"max_ms\":%.1f,\"px_per_s\":%lu,\"full_redraw_ms\":%.1f}\n",
             (unsigned long)p.fps, p.avg_ms, p.max_ms, (unsigned long)p.px_per_s,
             full_us < 0 ? -1.0 : full_us / 1000.0);
    httpd_resp_set_type(req, "application/json");
    return httpd_resp_sendstr(req, buf);
}

static void on_close(httpd_handle_t hd, int fd)
{
    if (fd == s_client) {
        s_client = -1;
        ESP_LOGI(TAG, "companion disconnected");
    }
    close(fd);
}

typedef struct {
    int fd;
    size_t len;
    char data[];
} send_job_t;

static void send_work(void *arg)
{
    send_job_t *job = arg;
    if (job->fd == s_client && job->fd >= 0) {
        httpd_ws_frame_t f = {
            .type = HTTPD_WS_TYPE_TEXT,
            .payload = (uint8_t *)job->data,
            .len = job->len,
            .final = true,
        };
        httpd_ws_send_frame_async(s_server, job->fd, &f);
    }
    free(job);
}

void hub_server_send(const char *json)
{
    int fd = s_client;
    if (!s_server || fd < 0) return;
    size_t len = strlen(json);
    send_job_t *job = malloc(sizeof(send_job_t) + len + 1);
    if (!job) return;
    job->fd = fd;
    job->len = len;
    memcpy(job->data, json, len + 1);
    if (httpd_queue_work(s_server, send_work, job) != ESP_OK) {
        free(job);
    }
}

bool hub_server_pc_online(void)
{
    return s_client >= 0 && (uint32_t)(now_ms() - s_last_rx_ms) < ONLINE_TIMEOUT_MS;
}

void hub_server_start(void)
{
    httpd_config_t cfg = HTTPD_DEFAULT_CONFIG();
    cfg.stack_size = 8192;
    cfg.close_fn = on_close;
    cfg.lru_purge_enable = true;
    cfg.max_open_sockets = 4;
    if (httpd_start(&s_server, &cfg) != ESP_OK) {
        ESP_LOGE(TAG, "http server failed to start");
        return;
    }
    static const httpd_uri_t ws = {
        .uri = "/ws", .method = HTTP_GET, .handler = ws_handler, .is_websocket = true,
    };
    static const httpd_uri_t root = {.uri = "/", .method = HTTP_GET, .handler = root_handler};
    httpd_register_uri_handler(s_server, &ws);
    httpd_register_uri_handler(s_server, &root);
    static const httpd_uri_t shot = {.uri = "/screenshot", .method = HTTP_GET, .handler = screenshot_handler};
    static const httpd_uri_t tap = {.uri = "/tap", .method = HTTP_GET, .handler = tap_handler};
    httpd_register_uri_handler(s_server, &shot);
    httpd_register_uri_handler(s_server, &tap);
    static const httpd_uri_t perf = {.uri = "/perf", .method = HTTP_GET, .handler = perf_handler};
    httpd_register_uri_handler(s_server, &perf);
    ota_register(s_server);
    pair_register(s_server);
    ESP_LOGI(TAG, "listening on :80");
}
