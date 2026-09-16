#include <stdint.h>

typedef struct LivewallDecoder LivewallDecoder;

typedef struct LivewallRgbFrame {
    uint8_t *data;
    int width;
    int height;
    int stride;
} LivewallRgbFrame;

typedef struct LivewallDmabufPlane {
    int fd;
    uint32_t offset;
    uint32_t stride;
} LivewallDmabufPlane;

typedef struct LivewallDmabufFrame {
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint64_t modifier;
    int nb_planes;
    LivewallDmabufPlane planes[4];
} LivewallDmabufFrame;

void livewall_decoder_set_target_size(LivewallDecoder *d, int w, int h);
LivewallDecoder *livewall_decoder_open(const char *path, const char *vaapi_device, int want_hw, int efficiency, int force_rgb);
void livewall_decoder_free(LivewallDecoder *d);
int livewall_decoder_is_hw(const LivewallDecoder *d);
float livewall_decoder_fps(const LivewallDecoder *d);
int livewall_decoder_width(const LivewallDecoder *d);
int livewall_decoder_height(const LivewallDecoder *d);
const char *livewall_decoder_warning(const LivewallDecoder *d);
int livewall_decoder_seek_start(LivewallDecoder *d);
int livewall_decoder_next(LivewallDecoder *d, LivewallRgbFrame *rgb, LivewallDmabufFrame *dma);
void livewall_rgb_free(LivewallRgbFrame *f);
void livewall_dmabuf_close(LivewallDmabufFrame *f);
