// Open-Meteo request URL, response parsing and WMO code -> icon/text. No network code, so the
// simulator (firmware/sim) shares it; the HTTP task is in weather.c.
#include "weather.h"

#include <stdio.h>
#include <string.h>
#include <time.h>

#include "cJSON.h"
#include "hubcfg.h"
#include "icons.h"

void weather_build_url(char *out, size_t n)
{
    const hub_cfg_t *c = hubcfg();
    snprintf(out, n,
             "https://api.open-meteo.com/v1/forecast?latitude=%s&longitude=%s"
             "&current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,is_day,wind_speed_10m"
             "&hourly=temperature_2m,weather_code,precipitation_probability"
             "&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max"
             "&timezone=auto&timeformat=unixtime&forecast_days=6&forecast_hours=24"
             // One model everywhere: GeoSphere Austria C-LAEF (1 km) for 60 h, then ECMWF; outside
             // C-LAEF's area ECMWF. Open-Meteo's default (ICON-D2 in Vienna) showed rain stations didn't.
             "&models=geosphere_seamless%s",
             c->lat, c->lon,
             c->fahrenheit ? "&temperature_unit=fahrenheit&wind_speed_unit=mph" : "&wind_speed_unit=kmh");
}

const char *weather_icon(int code, bool is_day)
{
    if (code == 0) return is_day ? ICON_WB_SUNNY : ICON_NIGHTS_STAY;
    if (code <= 2) return is_day ? ICON_WB_CLOUDY : ICON_NIGHTS_STAY;
    if (code == 3) return ICON_CLOUD;
    if (code == 45 || code == 48) return ICON_FOGGY;
    if (code >= 51 && code <= 57) return ICON_GRAIN;
    if ((code >= 61 && code <= 67) || (code >= 80 && code <= 82)) return ICON_UMBRELLA;
    if ((code >= 71 && code <= 77) || code == 85 || code == 86) return ICON_AC_UNIT;
    if (code >= 95) return ICON_THUNDERSTORM;
    return ICON_CLOUD;
}

const char *weather_text(int code)
{
    switch (code) {
    case 0: return "Clear";
    case 1: return "Mostly clear";
    case 2: return "Partly cloudy";
    case 3: return "Overcast";
    case 45: case 48: return "Fog";
    case 51: case 53: case 55: return "Drizzle";
    case 56: case 57: return "Freezing drizzle";
    case 61: return "Light rain";
    case 63: return "Rain";
    case 65: return "Heavy rain";
    case 66: case 67: return "Freezing rain";
    case 71: return "Light snow";
    case 73: return "Snow";
    case 75: return "Heavy snow";
    case 77: return "Snow grains";
    case 80: case 81: return "Showers";
    case 82: return "Heavy showers";
    case 85: case 86: return "Snow showers";
    case 95: return "Thunderstorm";
    case 96: case 99: return "Storm with hail";
    default: return "Unknown";
    }
}

static int fill_array_f(const cJSON *arr, float *out, int max)
{
    int n = 0;
    const cJSON *it;
    cJSON_ArrayForEach(it, arr) {
        if (n >= max) break;
        out[n++] = cJSON_IsNumber(it) ? (float)it->valuedouble : 0;
    }
    return n;
}

static int fill_array_i(const cJSON *arr, int *out, int max)
{
    int n = 0;
    const cJSON *it;
    cJSON_ArrayForEach(it, arr) {
        if (n >= max) break;
        out[n++] = cJSON_IsNumber(it) ? it->valueint : 0;
    }
    return n;
}

static int fill_array_t(const cJSON *arr, time_t *out, int max)
{
    int n = 0;
    const cJSON *it;
    cJSON_ArrayForEach(it, arr) {
        if (n >= max) break;
        out[n++] = cJSON_IsNumber(it) ? (time_t)it->valuedouble : 0;
    }
    return n;
}

// The weather code comes from the model run, the precipitation % from ensembles, so light amounts
// can show a rain or snow icon at 0 %. Below 20 % the icon shows Overcast, matching the %.
#define WEATHER_PRECIP_MIN_PCT 20

static int weather_shown_code(int code, int precip_pct)
{
    return (code >= 51 && precip_pct < WEATHER_PRECIP_MIN_PCT) ? 3 : code;
}

static double jnum(const cJSON *o, const char *k)
{
    const cJSON *v = cJSON_GetObjectItemCaseSensitive(o, k);
    return cJSON_IsNumber(v) ? v->valuedouble : 0;
}

bool weather_parse(const char *body, hub_weather_t *w)
{
    cJSON *root = cJSON_Parse(body);
    if (!root) return false;
    const cJSON *cur = cJSON_GetObjectItemCaseSensitive(root, "current");
    const cJSON *hourly = cJSON_GetObjectItemCaseSensitive(root, "hourly");
    const cJSON *daily = cJSON_GetObjectItemCaseSensitive(root, "daily");
    bool ok = cur && hourly && daily;
    if (ok) {
        memset(w, 0, sizeof(*w));
        w->valid = true;
        w->fetched = time(NULL);
        w->temp = jnum(cur, "temperature_2m");
        w->feels = jnum(cur, "apparent_temperature");
        w->humidity = jnum(cur, "relative_humidity_2m");
        w->code = jnum(cur, "weather_code");
        w->is_day = jnum(cur, "is_day") != 0;
        w->wind = jnum(cur, "wind_speed_10m");
        fill_array_t(cJSON_GetObjectItem(hourly, "time"), w->hour_t, HUB_HOURS);
        fill_array_f(cJSON_GetObjectItem(hourly, "temperature_2m"), w->hour_temp, HUB_HOURS);
        fill_array_i(cJSON_GetObjectItem(hourly, "weather_code"), w->hour_code, HUB_HOURS);
        fill_array_i(cJSON_GetObjectItem(hourly, "precipitation_probability"), w->hour_precip, HUB_HOURS);
        fill_array_t(cJSON_GetObjectItem(daily, "time"), w->day_t, HUB_DAYS);
        fill_array_f(cJSON_GetObjectItem(daily, "temperature_2m_max"), w->day_hi, HUB_DAYS);
        fill_array_f(cJSON_GetObjectItem(daily, "temperature_2m_min"), w->day_lo, HUB_DAYS);
        fill_array_i(cJSON_GetObjectItem(daily, "weather_code"), w->day_code, HUB_DAYS);
        fill_array_i(cJSON_GetObjectItem(daily, "precipitation_probability_max"), w->day_precip, HUB_DAYS);
        for (int i = 0; i < HUB_HOURS; i++) w->hour_code[i] = weather_shown_code(w->hour_code[i], w->hour_precip[i]);
        for (int i = 0; i < HUB_DAYS; i++) w->day_code[i] = weather_shown_code(w->day_code[i], w->day_precip[i]);
    }
    cJSON_Delete(root);
    return ok;
}

uint32_t weather_color(int code, bool is_day)
{
    if (code <= 1) return is_day ? 0xe3b866 : 0xb39ad9;
    if (code == 2) return is_day ? 0xe0c690 : 0xa3abd9;
    if (code == 3 || code == 45 || code == 48) return 0xa9b0c8;
    if ((code >= 71 && code <= 77) || code == 85 || code == 86) return 0xdfe6f2;
    if (code >= 95) return 0xb39ad9;
    return 0x7cc3d1; // drizzle and rain
}
