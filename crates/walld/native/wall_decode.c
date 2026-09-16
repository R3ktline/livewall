//! Minimal FFmpeg decode bridge (avoids ffmpeg-sys-next bindgen vs FFmpeg 9).

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/hwcontext.h>
#include <libavutil/hwcontext_drm.h>
#include <libavutil/imgutils.h>
#include <libswscale/swscale.h>

typedef struct WallDecoder {
    AVFormatContext *fmt;
    AVCodecContext *codec;
    AVBufferRef *hw_device_ctx;
    int stream_index;
    struct SwsContext *sws;
    int width;
    int height;
    float fps;
    int hw_vaapi;
    int force_rgb;
    int target_w;
    int target_h;
    char warning[256];
} WallDecoder;

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

static enum AVPixelFormat get_vaapi_format(AVCodecContext *ctx, const enum AVPixelFormat *pix_fmts) {
    (void)ctx;
    for (const enum AVPixelFormat *p = pix_fmts; *p != AV_PIX_FMT_NONE; p++) {
        if (*p == AV_PIX_FMT_VAAPI)
            return *p;
    }
    /* Allow software formats if VAAPI is not offered for this stream. */
    return pix_fmts[0];
}

WallDecoder *wall_decoder_open(const char *path, const char *vaapi_device, int want_hw, int efficiency,
                               int force_rgb) {
    WallDecoder *d = calloc(1, sizeof(*d));
    if (!d)
        return NULL;

    if (avformat_open_input(&d->fmt, path, NULL, NULL) < 0) {
        free(d);
        return NULL;
    }
    if (avformat_find_stream_info(d->fmt, NULL) < 0)
        goto fail;

    const AVCodec *codec = NULL;
    d->stream_index = av_find_best_stream(d->fmt, AVMEDIA_TYPE_VIDEO, -1, -1, &codec, 0);
    if (d->stream_index < 0 || !codec)
        goto fail;

    AVRational fr = d->fmt->streams[d->stream_index]->avg_frame_rate;
    if (fr.num == 0 || fr.den == 0)
        fr = d->fmt->streams[d->stream_index]->r_frame_rate;
    d->fps = (fr.den != 0) ? ((float)fr.num / (float)fr.den) : 30.0f;

    int opened = 0;
    if (want_hw) {
        d->codec = avcodec_alloc_context3(codec);
        if (!d->codec)
            goto fail;
        if (avcodec_parameters_to_context(d->codec, d->fmt->streams[d->stream_index]->codecpar) < 0)
            goto fail;
        if (av_hwdevice_ctx_create(&d->hw_device_ctx, AV_HWDEVICE_TYPE_VAAPI, vaapi_device, NULL, 0) == 0) {
            d->codec->hw_device_ctx = av_buffer_ref(d->hw_device_ctx);
            d->codec->get_format = get_vaapi_format;
            if (avcodec_open2(d->codec, codec, NULL) == 0) {
                d->hw_vaapi = 1;
                opened = 1;
            } else {
                avcodec_free_context(&d->codec);
                av_buffer_unref(&d->hw_device_ctx);
                d->hw_device_ctx = NULL;
                snprintf(d->warning, sizeof(d->warning),
                         "VA-API open failed for this codec; soft decode");
            }
        } else {
            snprintf(d->warning, sizeof(d->warning), "VA-API device unavailable; soft decode");
        }
        if (!opened && efficiency) {
            snprintf(d->warning, sizeof(d->warning), "VA-API unavailable (efficiency mode)");
            goto fail;
        }
    }

    if (!opened) {
        d->codec = avcodec_alloc_context3(codec);
        if (!d->codec)
            goto fail;
        if (avcodec_parameters_to_context(d->codec, d->fmt->streams[d->stream_index]->codecpar) < 0)
            goto fail;
        d->hw_vaapi = 0;
        if (avcodec_open2(d->codec, codec, NULL) < 0)
            goto fail;
        if (!d->warning[0])
            snprintf(d->warning, sizeof(d->warning), "software decode (higher CPU/RAM)");
    }

    d->width = d->codec->width;
    d->height = d->codec->height;
    d->force_rgb = force_rgb ? 1 : 0;
    d->target_w = 0;
    d->target_h = 0;
    if (d->force_rgb && d->hw_vaapi) {
        snprintf(d->warning, sizeof(d->warning),
                 "hybrid GPU: VA-API decode + SHM present (HDMI/dGPU safe)");
    }
    return d;

fail:
    if (d->codec)
        avcodec_free_context(&d->codec);
    if (d->hw_device_ctx)
        av_buffer_unref(&d->hw_device_ctx);
    if (d->fmt)
        avformat_close_input(&d->fmt);
    free(d);
    return NULL;
}

int wall_decoder_is_hw(const WallDecoder *d) { return d && d->hw_vaapi; }
float wall_decoder_fps(const WallDecoder *d) { return d ? d->fps : 0.0f; }
int wall_decoder_width(const WallDecoder *d) { return d ? d->width : 0; }
int wall_decoder_height(const WallDecoder *d) { return d ? d->height : 0; }

void wall_decoder_set_target_size(WallDecoder *d, int w, int h) {
    if (!d)
        return;
    if (w < 16)
        w = 0;
    if (h < 16)
        h = 0;
    /* Even dimensions for YUV-friendly scaling */
    if (w > 0)
        w &= ~1;
    if (h > 0)
        h &= ~1;
    if (d->target_w != w || d->target_h != h) {
        d->target_w = w;
        d->target_h = h;
        if (d->sws) {
            sws_freeContext(d->sws);
            d->sws = NULL;
        }
    }
}
const char *wall_decoder_warning(const WallDecoder *d) {
    return (d && d->warning[0]) ? d->warning : NULL;
}

int wall_decoder_seek_start(WallDecoder *d) {
    if (!d)
        return -1;
    int ret = avformat_seek_file(d->fmt, -1, INT64_MIN, 0, INT64_MAX, AVSEEK_FLAG_BACKWARD);
    avcodec_flush_buffers(d->codec);
    return ret;
}

static int export_drm(AVFrame *frame, WallDmabufFrame *out) {
    AVFrame *mapped = av_frame_alloc();
    if (!mapped)
        return -1;
    mapped->format = AV_PIX_FMT_DRM_PRIME;
    int err = av_hwframe_map(mapped, frame, AV_HWFRAME_MAP_READ);
    if (err < 0) {
        av_frame_free(&mapped);
        return err;
    }
    if (mapped->format != AV_PIX_FMT_DRM_PRIME || !mapped->data[0]) {
        av_frame_free(&mapped);
        return -1;
    }
    const AVDRMFrameDescriptor *desc = (const AVDRMFrameDescriptor *)mapped->data[0];
    if (desc->nb_layers <= 0 || desc->nb_objects <= 0) {
        av_frame_free(&mapped);
        return -1;
    }
    const AVDRMLayerDescriptor *layer = &desc->layers[0];
    out->width = (uint32_t)mapped->width;
    out->height = (uint32_t)mapped->height;
    out->format = (uint32_t)layer->format;
    out->nb_planes = layer->nb_planes;
    out->modifier = 0x00ffffffffffffffULL;
    for (int i = 0; i < layer->nb_planes && i < 4; i++) {
        const AVDRMPlaneDescriptor *p = &layer->planes[i];
        const AVDRMObjectDescriptor *obj = &desc->objects[p->object_index];
        out->modifier = obj->format_modifier;
        out->planes[i].fd = dup(obj->fd);
        out->planes[i].offset = (uint32_t)p->offset;
        out->planes[i].stride = (uint32_t)p->pitch;
    }
    av_frame_free(&mapped);
    return 0;
}

static int to_rgb(WallDecoder *d, AVFrame *src, WallRgbFrame *out) {
    AVFrame *sw = src;
    AVFrame *tmp = NULL;
    if (src->format == AV_PIX_FMT_VAAPI || src->format == AV_PIX_FMT_DRM_PRIME) {
        tmp = av_frame_alloc();
        if (!tmp)
            return -1;
        if (av_hwframe_transfer_data(tmp, src, 0) < 0) {
            av_frame_free(&tmp);
            return -1;
        }
        sw = tmp;
    }

    enum AVPixelFormat src_fmt = (enum AVPixelFormat)sw->format;
    int dst_w = sw->width;
    int dst_h = sw->height;
    if (d->target_w > 0 && d->target_h > 0) {
        dst_w = d->target_w;
        dst_h = d->target_h;
    }

    /* BGR0 = little-endian XRGB8888 for wl_shm */
    d->sws = sws_getCachedContext(d->sws, sw->width, sw->height, src_fmt, dst_w, dst_h,
                                  AV_PIX_FMT_BGR0, SWS_FAST_BILINEAR, NULL, NULL, NULL);
    if (!d->sws) {
        if (tmp)
            av_frame_free(&tmp);
        return -1;
    }

    int stride = dst_w * 4;
    uint8_t *buf = (uint8_t *)malloc((size_t)stride * (size_t)dst_h);
    if (!buf) {
        if (tmp)
            av_frame_free(&tmp);
        return -1;
    }
    uint8_t *dst_slices[4] = {buf, NULL, NULL, NULL};
    int dst_stride[4] = {stride, 0, 0, 0};
    sws_scale(d->sws, (const uint8_t *const *)sw->data, sw->linesize, 0, sw->height, dst_slices,
              dst_stride);
    out->data = buf;
    out->width = dst_w;
    out->height = dst_h;
    out->stride = stride;
    if (tmp)
        av_frame_free(&tmp);
    return 0;
}

/* returns: 1=rgb, 2=dmabuf, 0=eof, -1=error */
int wall_decoder_next(WallDecoder *d, WallRgbFrame *rgb, WallDmabufFrame *dma) {
    if (!d)
        return -1;
    AVPacket *pkt = av_packet_alloc();
    AVFrame *frame = av_frame_alloc();
    if (!pkt || !frame) {
        av_packet_free(&pkt);
        av_frame_free(&frame);
        return -1;
    }

    int got = 0;
    while (!got) {
        int ret = av_read_frame(d->fmt, pkt);
        if (ret < 0) {
            avcodec_send_packet(d->codec, NULL);
            ret = avcodec_receive_frame(d->codec, frame);
            if (ret == AVERROR_EOF) {
                av_packet_free(&pkt);
                av_frame_free(&frame);
                return 0;
            }
            if (ret < 0) {
                av_packet_free(&pkt);
                av_frame_free(&frame);
                return -1;
            }
            got = 1;
            break;
        }
        if (pkt->stream_index != d->stream_index) {
            av_packet_unref(pkt);
            continue;
        }
        if (avcodec_send_packet(d->codec, pkt) < 0) {
            av_packet_unref(pkt);
            continue;
        }
        av_packet_unref(pkt);
        ret = avcodec_receive_frame(d->codec, frame);
        if (ret == AVERROR(EAGAIN))
            continue;
        if (ret == AVERROR_EOF) {
            av_packet_free(&pkt);
            av_frame_free(&frame);
            return 0;
        }
        if (ret < 0) {
            av_packet_free(&pkt);
            av_frame_free(&frame);
            return -1;
        }
        got = 1;
    }

    int result = -1;
    if (d->hw_vaapi && frame->format == AV_PIX_FMT_VAAPI && !d->force_rgb) {
        if (export_drm(frame, dma) == 0) {
            result = 2;
        } else {
            /* fall soft for this frame */
            if (to_rgb(d, frame, rgb) == 0)
                result = 1;
        }
    } else {
        if (to_rgb(d, frame, rgb) == 0)
            result = 1;
    }

    av_packet_free(&pkt);
    av_frame_free(&frame);
    return result;
}

void wall_rgb_free(WallRgbFrame *f) {
    if (f && f->data) {
        free(f->data);
        f->data = NULL;
    }
}

void wall_dmabuf_close(WallDmabufFrame *f) {
    if (!f)
        return;
    for (int i = 0; i < f->nb_planes && i < 4; i++) {
        if (f->planes[i].fd >= 0) {
            close(f->planes[i].fd);
            f->planes[i].fd = -1;
        }
    }
}

void wall_decoder_free(WallDecoder *d) {
    if (!d)
        return;
    if (d->sws)
        sws_freeContext(d->sws);
    if (d->codec)
        avcodec_free_context(&d->codec);
    if (d->hw_device_ctx)
        av_buffer_unref(&d->hw_device_ctx);
    if (d->fmt)
        avformat_close_input(&d->fmt);
    free(d);
}
