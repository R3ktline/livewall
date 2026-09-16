#include <stdint.h>

typedef struct WallDecoder WallDecoder;

typedef struct WallRgbFrame {
    uint8_t *data;
    int width;
    int height;
    int stride;
} WallRgbFrame;

typedef struct WallDmabufPlane {
    int fd;
    uint32_t offset;
    uint32_t stride;
} WallDmabufPlane;

typedef struct WallDmabufFrame {
    uint32_t width;
    uint32_t height;
    uint32_t format;
    uint64_t modifier;
    int nb_planes;
    WallDmabufPlane planes[4];
} WallDmabufFrame;

void wall_decoder_set_target_size(WallDecoder *d, int w, int h);
WallDecoder *wall_decoder_open(const char *path, const char *vaapi_device, int want_hw, int efficiency, int force_rgb);
void wall_decoder_free(WallDecoder *d);
int wall_decoder_is_hw(const WallDecoder *d);
float wall_decoder_fps(const WallDecoder *d);
int wall_decoder_width(const WallDecoder *d);
int wall_decoder_height(const WallDecoder *d);
const char *wall_decoder_warning(const WallDecoder *d);
int wall_decoder_seek_start(WallDecoder *d);
int wall_decoder_next(WallDecoder *d, WallRgbFrame *rgb, WallDmabufFrame *dma);
void wall_rgb_free(WallRgbFrame *f);
void wall_dmabuf_close(WallDmabufFrame *f);
