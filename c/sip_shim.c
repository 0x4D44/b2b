// Minimal baresip integration layer for Sink role
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <re.h>
#include <rem.h>
#include <baresip.h>
#include <stdio.h>
#include <math.h>
#include <inttypes.h>

typedef void (*b2b_pcm_cb)(const int16_t* samples, size_t nsamples, void* user);

static b2b_pcm_cb g_cb = 0;
static void* g_user = 0;
static struct ua* g_ua = NULL;
static struct aufilt g_tap;
static struct tmr g_aa_tmr;
static struct tmr g_sink_m_tmr;
static struct tmr g_mx_m_tmr;
static struct log g_log;
static const char *g_role = NULL;
// forward decl for sink metrics tick
static void sink_metrics_tick(void *arg);
static struct ausrc *g_src = NULL;
static struct aubuf *g_src_ab = NULL;
static uint32_t g_src_srate = 8000;
static uint8_t  g_src_ch = 1;
static uint32_t g_src_ptime = 20;
static size_t   g_src_sampc = 160; // 20ms @ 8kHz mono
static volatile bool g_src_started = false;
static struct { uint64_t last_ms; uint32_t pkt; uint32_t min_int; uint32_t max_int; } g_src_m;

struct b2b_src_st {
    struct ausrc_prm prm;
    ausrc_read_h *rh;
    ausrc_error_h *errh;
    void *arg;
    bool run;
    thrd_t th;
};

static int b2b_src_thread(void *arg)
{
    struct b2b_src_st *st = arg;
    int16_t *sampv = mem_alloc(g_src_sampc * sizeof(int16_t), NULL);
    if (!sampv) return ENOMEM;
    uint64_t next = tmr_jiffies();
    while (st->run) {
        if (!g_src_started) { sys_msleep(5); next = tmr_jiffies(); continue; }
        uint64_t now = tmr_jiffies();
        if (now + 1 < next) { // sleep until next tick (1ms slack)
            sys_msleep((unsigned)(next - now));
            continue;
        }
        // Produce as many frames as needed to catch up
        do {
            struct auframe af;
            auframe_init(&af, AUFMT_S16LE, sampv, g_src_sampc, g_src_srate, g_src_ch);
            aubuf_read_auframe(g_src_ab, &af);
            st->rh(&af, st->arg);
            next += g_src_ptime;
            now = tmr_jiffies();
            /* metrics update */
            if (g_src_m.last_ms) {
                uint32_t d = (uint32_t)(now - g_src_m.last_ms);
                if (g_src_m.min_int == 0 || d < g_src_m.min_int) g_src_m.min_int = d;
                if (d > g_src_m.max_int) g_src_m.max_int = d;
            }
            g_src_m.last_ms = now;
            g_src_m.pkt++;
            if ((g_src_m.pkt % 250) == 0) {
                size_t cur = g_src_ab ? aubuf_cur_size(g_src_ab) : 0;
                size_t bytes_per_ms = (g_src_srate * g_src_ch * 2) / 1000;
                uint32_t back_ms = bytes_per_ms ? (uint32_t)(cur / bytes_per_ms) : 0;
                re_printf("SRC_METRICS5s pkts=250 int_min=%ums int_max=%ums backlog_ms=%u\n",
                          g_src_m.min_int, g_src_m.max_int, back_ms);
                fflush(NULL);
                g_src_m.min_int = 0; g_src_m.max_int = 0;
            }
        } while (next <= now);
    }
    mem_deref(sampv);
    return 0;
}

static void b2b_src_destructor(void *arg)
{
    struct b2b_src_st *st = arg;
    st->run = false;
    if (st->th) thrd_join(st->th, NULL);
}

static int b2b_src_alloc(struct ausrc_st **stp, const struct ausrc *as,
                         struct ausrc_prm *prm, const char *device,
                         ausrc_read_h *rh, ausrc_error_h *errh, void *arg)
{
    (void)as; (void)device;
    if (!stp || !prm || !rh) return EINVAL;
    struct b2b_src_st *st = mem_zalloc(sizeof(*st), b2b_src_destructor);
    if (!st) return ENOMEM;
    st->prm = *prm;
    st->rh = rh;
    st->errh = errh;
    st->arg = arg;
    st->run = true;
    g_src_srate = prm->srate ? prm->srate : g_src_srate;
    g_src_ch = prm->ch ? prm->ch : g_src_ch;
    g_src_ptime = prm->ptime ? prm->ptime : g_src_ptime;
    g_src_sampc = (g_src_srate * g_src_ch * g_src_ptime) / 1000;
    if (!g_src_ab) {
        int err = aubuf_alloc(&g_src_ab, 0, 0);
        if (err) { mem_deref(st); return err; }
    }
    if (0 != thread_create_name(&st->th, "b2b_src", b2b_src_thread, st)) {
        mem_deref(st);
        return ENOMEM;
    }
    *stp = (struct ausrc_st *)st;
    return 0;
}


struct b2b_dec_st { struct aufilt_dec_st af; };

static int decupd(struct aufilt_dec_st **stp, void **ctx, const struct aufilt *af,
                  struct aufilt_prm *prm, const struct audio *au)
{
    (void)ctx; (void)af; (void)prm; (void)au;
    struct b2b_dec_st *st = mem_zalloc(sizeof(*st), NULL);
    if (!st) return ENOMEM;
    *stp = (struct aufilt_dec_st *)st;
    return 0;
}

static int dech(struct aufilt_dec_st *st, struct auframe *af)
{
    (void)st;
    if (!g_cb || !af || !af->sampv || af->fmt != AUFMT_S16LE)
        return 0;
    // Deliver mono/8k frames as provided by the audio pipeline
    g_cb((const int16_t*)af->sampv, af->sampc, g_user);
    // count frames for sink metrics (one decoded frame per 20ms)
    if (g_role && strcmp(g_role, "SINK") == 0) {
        extern void sink_metrics_count_frame(void);
        sink_metrics_count_frame();
    }
    return 0;
}

static void reg_filter(void)
{
    memset(&g_tap, 0, sizeof(g_tap));
    g_tap.name = "b2b_tap";
    g_tap.decupdh = decupd;
    g_tap.dech = dech;
    aufilt_register(baresip_aufiltl(), &g_tap);
}

static void log_adapter(uint32_t level, const char *msg)
{
    (void)level;
    if (!msg) return;
    /* Do not prefix; the Rust orchestrator tags + timestamps every line. */
    (void)re_printf("%s", msg);
    /* ensure line is flushed promptly even when not attached to a TTY */
    fflush(NULL);
    // crude drop detector for sink
    if (g_role && strcmp(g_role, "SINK") == 0) {
        if (strstr(msg, "jbuf: drop")) {
            extern void sink_metrics_count_drop(void);
            sink_metrics_count_drop();
        }
    }
}

static void aa_tick(void *arg)
{
    (void)arg;
    if (g_ua) {
        struct call *c = (struct call *)ua_call(g_ua);
        if (c) {
            int st = call_state(c);
            if (st == CALL_STATE_INCOMING || st == CALL_STATE_RINGING || st == CALL_STATE_EARLY) {
                (void)ua_answer(g_ua, c, VIDMODE_OFF);
            }
        }
    }
    tmr_start(&g_aa_tmr, 50, aa_tick, NULL);
}

// Configure SIP listen and autoanswer.
static int configure(const char* bind_addr)
{
    // Create a tiny config with our listen address and auto-accept enabled.
    char buf[512];
    if (bind_addr && *bind_addr) {
        int n = re_snprintf(buf, sizeof(buf),
                            "sip_listen\t%s\n"
                            "call_accept\tyes\n",
                            bind_addr);
        if (n < 0) return EINVAL;
        int rc = conf_configure_buf((const uint8_t*)buf, (size_t)n);
        if (rc) return rc;
    }
    return 0;
}

int sip_sink_init(const char* bind_addr)
{
    int err = 0;
    g_role = "SINK";
    g_log.h = log_adapter;
    log_register_handler(&g_log);
    /* Silence default stdout logger and info noise; orchestrator adds context */
    log_enable_stdout(false);
    log_enable_timestamps(false);
    log_enable_color(false);
    log_enable_info(false);
    // Register our decode tap so we receive PCM after codec decode.
    reg_filter();

    // Configure listen address (optional); then create a catch-all UA that auto-answers.
    err |= configure(bind_addr);

    // Allocate a UA with a dummy AOR; it will act as UAS and auto-answer.
    if (!g_ua) {
        err |= ua_alloc(&g_ua, "sip:anon@0.0.0.0;regint=0;catchall=yes;audio_codecs=pcmu");
        if (err) return err;
        (void)ua_set_autoanswer_value(g_ua, "yes");
        ua_set_catchall(g_ua, true);
    }
    // Start auto-answer tick
    tmr_start(&g_aa_tmr, 50, aa_tick, NULL);
    // Start sink metrics tick (5s)
    tmr_start(&g_sink_m_tmr, 5000, sink_metrics_tick, NULL);
    return err;
}

int sip_sink_set_pcm_callback(b2b_pcm_cb cb, void* user) { g_cb = cb; g_user = user; return 0; }

int sip_sink_shutdown(void)
{
    if (g_ua) { ua_destroy(g_ua); g_ua = NULL; }
    tmr_cancel(&g_aa_tmr);
    tmr_cancel(&g_sink_m_tmr);
    if (g_tap.name) { aufilt_unregister(&g_tap); memset(&g_tap, 0, sizeof(g_tap)); }
    g_cb = 0; g_user = 0;
    return 0;
}

// Source (outbound) APIs
int sip_source_start(const char* target_uri, uint32_t srate, uint8_t ch, uint32_t ptime_ms)
{
    int err = 0;
    g_role = "SRC ";
    g_log.h = log_adapter;
    log_register_handler(&g_log);
    /* Same logging normalization for Source */
    log_enable_stdout(false);
    log_enable_timestamps(false);
    log_enable_color(false);
    log_enable_info(false);
    g_src_srate = srate ? srate : g_src_srate;
    g_src_ch = ch ? ch : g_src_ch;
    g_src_ptime = ptime_ms ? ptime_ms : g_src_ptime;
    g_src_sampc = (g_src_srate * g_src_ch * g_src_ptime) / 1000;

    // Configure audio to use our ausrc and 8k mono s16
    char buf[256];
    re_snprintf(buf, sizeof(buf),
        "audio_source\t\tb2b_src,\n"
        "ausrc_srate\t\t%u\n"
        "ausrc_channels\t\t%u\n"
        "ausrc_format\t\ts16\n",
        g_src_srate, g_src_ch);
    err |= conf_configure_buf((const uint8_t*)buf, strlen(buf));

    // Register our ausrc under name b2b_src
    if (!g_src) {
        err |= ausrc_register(&g_src, baresip_ausrcl(), "b2b_src",
                              b2b_src_alloc);
        if (err) return err;
    }

    // Create a UA if needed and dial target
    if (!g_ua) {
        err |= ua_alloc(&g_ua, "sip:anon@0.0.0.0;regint=0;audio_codecs=pcmu");
        if (err) return err;
    }
    if (target_uri && *target_uri) {
        struct call *call = NULL;
        /* derive a from-URI using the target host so UA can select an laddr */
        char from[128] = {0};
        const char* p = target_uri;
        if (0 == strncmp(p, "sip:", 4)) p += 4;
        const char* host = p;
        /* strip optional user@ */
        const char* at = strchr(host, '@');
        if (at) host = at + 1;
        /* stop before last ':' (port) if present */
        const char* last_colon = strrchr(host, ':');
        size_t host_len = last_colon ? (size_t)(last_colon - host) : strlen(host);
        if (host_len > sizeof(from) - 16) host_len = sizeof(from) - 16;
        memcpy(from, "sip:anon@", 9);
        memcpy(from + 9, host, host_len);
        from[9 + host_len] = '\0';
        err |= ua_connect(g_ua, &call, from, target_uri, VIDMODE_OFF);
    }
    return err;
}

int sip_source_push_pcm(const int16_t* samples, size_t nsamples)
{
    if (!samples || nsamples == 0) return 0;
    if (!g_src_ab) return ENODEV;
    // Append frame to aubuf
    struct mbuf *mb = mbuf_alloc(nsamples * 2);
    if (!mb) return ENOMEM;
    mbuf_write_mem(mb, (const uint8_t*)samples, nsamples * 2);
    mb->pos = 0;
    struct auframe af;
    auframe_init(&af, AUFMT_S16LE, NULL, nsamples, g_src_srate, g_src_ch);
    int err = aubuf_append_auframe(g_src_ab, mb, &af);
    mem_deref(mb);
    return err;
}

int sip_source_tx_enable(int enable)
{
    g_src_started = enable ? true : false;
    return 0;
}

int sip_source_shutdown(void)
{
    if (g_src) { mem_deref(g_src); g_src = NULL; }
    if (g_src_ab) { mem_deref(g_src_ab); g_src_ab = NULL; }
    return 0;
}
int sip_source_backlog_ms(void)
{
    if (!g_src_ab) return 0;
    size_t cur = aubuf_cur_size(g_src_ab);
    size_t bytes_per_ms = (g_src_srate * g_src_ch * 2) / 1000;
    if (!bytes_per_ms) return 0;
    return (int)(cur / bytes_per_ms);
}
int sip_preconfigure_listen(const char* bind_addr)
{
    if (!bind_addr || !*bind_addr) return 0;
    char buf[512];
    int n = re_snprintf(buf, sizeof(buf),
                        "sip_listen\t%s\n"
                        "module\t\tg711\n"
                        "call_accept\tyes\n",
                        bind_addr);
    if (n < 0) return EINVAL;
    return conf_configure_buf((const uint8_t*)buf, (size_t)n);
}
// Return a CSV of compiled-in audio codecs, e.g. "pcmu/8000/1,pcma/8000/1,l16/8000/1"
const char* brs_codecs_csv(void)
{
    static char buf[512];
    size_t off = 0;
    struct le *le;
    buf[0] = '\0';
    for (le = list_head(baresip_aucodecl()); le; le = le->next) {
        struct aucodec *ac = le->data;
        if (!ac || !ac->name) continue;
        int n = re_snprintf(buf + off, sizeof(buf) - off, "%s%s/%u/%u",
                            off ? "," : "", ac->name, ac->srate, ac->ch);
        if (n < 0) break;
        off += (size_t)n;
        if (off >= sizeof(buf)) break;
    }
    return buf;
}

// --------------------- MIXER (bridge) ----------------------
static struct ua *g_mx_in = NULL;   // inbound UA (server)
static struct ua *g_mx_out = NULL;  // outbound UA (client)
static struct ausrc *g_mx_src = NULL; // ausrc for outbound leg
static struct auplay *g_mx_play = NULL; // custom auplay that captures inbound PCM
static bool g_mx_play_registered = false;
static struct list g_mx_legs = LIST_INIT;
static mtx_t g_mx_lock;
static bool g_mx_lock_ready = false;
static uint32_t g_mx_srate = 8000;
static uint8_t  g_mx_ch = 1;
static uint32_t g_mx_ptime = 20;
static size_t   g_mx_sampc = 160;
// DTMF/mix state
static const char *g_mx_dtmf_seq = "123#";
static char g_mx_dtmf_buf[128];
static size_t g_mx_dtmf_len = 4;
static size_t g_mx_dtmf_idx = 0;
static uint32_t g_mx_dtmf_period_ms = 1000;
static uint32_t g_mx_dtmf_elapsed_ms = 0;
static uint32_t g_mx_dtmf_off_ms = 50;     // inter-digit silence within period
static uint32_t g_mx_dtmf_pause_ms = 1200; // pause for '+' digit
static double g_mx_gain_in = 0.5;    // 0..1
static double g_mx_gain_dtmf = 0.5;  // 0..1
static double g_mx_ph1 = 0.0, g_mx_ph2 = 0.0;
static double g_mx_inc1 = 0.0, g_mx_inc2 = 0.0;

#define MX_MAX_BACKLOG_MS 250
#define MX_PRELOAD_FRAMES 6
#define MX_PRIME_EXTRA_FRAMES 3

struct mx_play_st;

static struct {
    uint32_t in_frames5s;
    uint32_t out_frames5s;
    uint32_t tone_on5s;
    uint32_t in_silence5s;
    uint32_t in_underrun5s;
    uint64_t in_samples5s;
    uint64_t out_samples5s;
    uint32_t bridge_ms_min;
    uint32_t bridge_ms_max;
} g_mx_m;
static bool g_mx_first_in = false;

struct mx_leg_ctx {
    struct le node;
    struct aubuf *buf;
    struct mx_play_st *play;
};

struct mx_play_st {
    struct auplay_prm prm;
    auplay_write_h *wh;
    void *arg;
    bool run;
    thrd_t th;
    struct mx_leg_ctx *leg;
    bool primed;
    uint32_t preload_frames;
};

struct mx_src_st {
    struct ausrc_prm prm;
    ausrc_read_h *rh;
    ausrc_error_h *errh;
    void *arg;
    bool run;
    thrd_t th;
};

static void mx_lock_init(void);
static void mx_leg_remove_all(void);
static void mx_metrics_tick(void *arg);

static bool mx_dtmf_lookup(char digit, double *f1, double *f2)
{
    static const struct { char digit; double f1; double f2; } table[] = {
        {'1', 697.0, 1209.0},
        {'2', 697.0, 1336.0},
        {'3', 697.0, 1477.0},
        {'A', 697.0, 1633.0},
        {'4', 770.0, 1209.0},
        {'5', 770.0, 1336.0},
        {'6', 770.0, 1477.0},
        {'B', 770.0, 1633.0},
        {'7', 852.0, 1209.0},
        {'8', 852.0, 1336.0},
        {'9', 852.0, 1477.0},
        {'C', 852.0, 1633.0},
        {'*', 941.0, 1209.0},
        {'0', 941.0, 1336.0},
        {'#', 941.0, 1477.0},
        {'D', 941.0, 1633.0},
    };
    for (size_t i = 0; i < sizeof(table)/sizeof(table[0]); ++i) {
        if (table[i].digit == digit) {
            if (f1) *f1 = table[i].f1;
            if (f2) *f2 = table[i].f2;
            return true;
        }
    }
    return false;
}

static void mx_dtmf_select_digit(char digit, uint32_t srate)
{
    double f1 = 0.0, f2 = 0.0;
    if (!mx_dtmf_lookup(digit, &f1, &f2) || !srate) {
        g_mx_inc1 = g_mx_inc2 = 0.0;
        g_mx_ph1 = g_mx_ph2 = 0.0;
        return;
    }
    g_mx_inc1 = 2.0 * M_PI * f1 / (double)srate;
    g_mx_inc2 = 2.0 * M_PI * f2 / (double)srate;
    g_mx_ph1 = g_mx_ph2 = 0.0;
}

static void mx_lock_init(void)
{
    if (g_mx_lock_ready)
        return;
    if (mtx_init(&g_mx_lock, mtx_plain) == thrd_success) {
        list_init(&g_mx_legs);
        g_mx_lock_ready = true;
    }
}

static void mx_leg_unlink(struct mx_leg_ctx *leg)
{
    if (!leg)
        return;
    if (leg->node.list) {
        if (g_mx_lock_ready)
            mtx_lock(&g_mx_lock);
        list_unlink(&leg->node);
        if (g_mx_lock_ready)
            mtx_unlock(&g_mx_lock);
    }
}

static void mx_leg_destructor(void *arg)
{
    struct mx_leg_ctx *leg = arg;
    mx_leg_unlink(leg);
    leg->buf = mem_deref(leg->buf);
}

static void mx_play_stop(struct mx_play_st *st)
{
    if (!st)
        return;
    st->run = false;
    if (st->th)
        thrd_join(st->th, NULL);
    if (st->leg) {
        st->leg->play = NULL;
        mem_deref(st->leg);
        st->leg = NULL;
    }
}

static int mx_play_thread(void *arg)
{
    struct mx_play_st *st = arg;
    uint32_t srate = st->prm.srate ? st->prm.srate : 8000;
    uint8_t ch = st->prm.ch ? st->prm.ch : 1;
    uint32_t ptime = st->prm.ptime ? st->prm.ptime : 20;
    size_t sampc = (size_t)(srate * ch * ptime / 1000);
    if (!sampc) sampc = 160;
    int16_t *buf = mem_alloc(sizeof(int16_t) * sampc, NULL);
    if (!buf)
        return ENOMEM;

    size_t frame_bytes = sampc * sizeof(int16_t);
    size_t prime_target_bytes = frame_bytes;
    unsigned preload_frames = st->preload_frames ? st->preload_frames : MX_PRELOAD_FRAMES;
    if (preload_frames > 32)
        preload_frames = 32;
    if (st->leg && st->leg->buf && preload_frames > 0) {
        if (g_mx_lock_ready)
            mtx_lock(&g_mx_lock);
        memset(buf, 0, frame_bytes);
        struct auframe pref;
        auframe_init(&pref, AUFMT_S16LE, buf, sampc, srate, ch);
        for (unsigned n = 0; n < preload_frames; ++n)
            (void)aubuf_write_auframe(st->leg->buf, &pref);
        st->primed = false;
        if (g_mx_lock_ready)
            mtx_unlock(&g_mx_lock);
    }
    if (preload_frames < MX_PRIME_EXTRA_FRAMES)
        prime_target_bytes = frame_bytes * (MX_PRIME_EXTRA_FRAMES + 1);
    else
        prime_target_bytes = frame_bytes * (preload_frames + MX_PRIME_EXTRA_FRAMES);

    while (st->run) {
        struct auframe af;
        auframe_init(&af, AUFMT_S16LE, buf, sampc, srate, ch);
        st->wh(&af, st->arg);

        if (st->leg && st->leg->buf) {
            size_t bytes_per_ms = (srate * ch * 2) / 1000;
            if (g_mx_lock_ready)
                mtx_lock(&g_mx_lock);
            if (st->leg && st->leg->buf) {
                (void)aubuf_write_auframe(st->leg->buf, &af);
                if (bytes_per_ms) {
                    size_t limit = bytes_per_ms * MX_MAX_BACKLOG_MS;
                    while (aubuf_cur_size(st->leg->buf) > limit) {
                        struct auframe drop;
                        auframe_init(&drop, AUFMT_S16LE, NULL, sampc, srate, ch);
                        aubuf_drop_auframe(st->leg->buf, &drop);
                    }
                    if (!st->primed) {
                        size_t cur_bytes = aubuf_cur_size(st->leg->buf);
                        if (cur_bytes >= prime_target_bytes)
                            st->primed = true;
                    }
                }
            }
            if (g_mx_lock_ready)
                mtx_unlock(&g_mx_lock);
        }

        sys_msleep(ptime ? ptime : 20);
    }

    mem_deref(buf);
    return 0;
}

static void mx_play_destructor(void *arg)
{
    struct mx_play_st *st = arg;
    mx_play_stop(st);
}

static int mx_play_alloc(struct auplay_st **stp, const struct auplay *ap,
                         struct auplay_prm *prm, const char *device,
                         auplay_write_h *wh, void *arg)
{
    (void)ap; (void)device;
    if (!stp || !prm || !wh)
        return EINVAL;

    struct mx_play_st *st = mem_zalloc(sizeof(*st), mx_play_destructor);
    if (!st)
        return ENOMEM;

    st->prm = *prm;
    if (st->prm.fmt != AUFMT_S16LE)
        st->prm.fmt = AUFMT_S16LE;
    if (!st->prm.srate)
        st->prm.srate = g_mx_srate ? g_mx_srate : 8000;
    if (!st->prm.ch)
        st->prm.ch = 1;
    if (!st->prm.ptime)
        st->prm.ptime = 20;

    st->wh = wh;
    st->arg = arg;
    st->run = true;
    st->primed = false;
    st->preload_frames = MX_PRELOAD_FRAMES;

    mx_lock_init();

    struct mx_leg_ctx *leg = mem_zalloc(sizeof(*leg), mx_leg_destructor);
    if (!leg) {
        mem_deref(st);
        return ENOMEM;
    }
    leg->play = st;
    if (aubuf_alloc(&leg->buf, 0, 0)) {
        mem_deref(leg);
        mem_deref(st);
        return ENOMEM;
    }
    st->leg = leg;

    if (g_mx_lock_ready) {
        mtx_lock(&g_mx_lock);
        list_append(&g_mx_legs, &leg->node, leg);
        mtx_unlock(&g_mx_lock);
    }

    if (0 != thread_create_name(&st->th, "mx_play", mx_play_thread, st)) {
        mem_deref(st);
        return ENOMEM;
    }

    *stp = (struct auplay_st *)st;
    return 0;
}

static int mx_src_thread(void *arg)
{
    struct mx_src_st *st = arg;
    int16_t *mixv = NULL;
    int16_t *tmp = NULL;
    int32_t *acc = NULL;
    size_t alloc_sampc = 0;
    uint64_t next = tmr_jiffies();

    while (st->run) {
        uint32_t srate = g_mx_srate ? g_mx_srate : 8000;
        uint8_t ch = g_mx_ch ? g_mx_ch : 1;
        uint32_t ptime = g_mx_ptime ? g_mx_ptime : 20;
        size_t sampc = (size_t)(srate * ch * ptime / 1000);
        if (!sampc) sampc = 160;

        if (sampc != alloc_sampc) {
            mixv = mem_deref(mixv);
            tmp = mem_deref(tmp);
            acc = mem_deref(acc);
            alloc_sampc = sampc;
            mixv = mem_alloc(sizeof(int16_t) * alloc_sampc, NULL);
            tmp = mem_alloc(sizeof(int16_t) * alloc_sampc, NULL);
            acc = mem_alloc(sizeof(int32_t) * alloc_sampc, NULL);
            if (!mixv || !tmp || !acc) {
                mixv = mem_deref(mixv);
                tmp = mem_deref(tmp);
                acc = mem_deref(acc);
                alloc_sampc = 0;
                sys_msleep(5);
                continue;
            }
        }

        size_t need_bytes = alloc_sampc * sizeof(int16_t);
        size_t bytes_per_ms = (srate * ch * 2) / 1000;

        if (g_mx_lock_ready && need_bytes) {
            uint64_t wait_deadline = tmr_jiffies() + (ptime ? ptime : 20);
            bool ready = false;
            while (st->run && !ready) {
                mtx_lock(&g_mx_lock);
                for (struct le *probe = list_head(&g_mx_legs); probe; probe = probe->next) {
                    struct mx_leg_ctx *pleg = probe->data;
                    if (!pleg || !pleg->buf)
                        continue;
                    if (!pleg->play || !pleg->play->primed)
                        continue;
                    if (aubuf_cur_size(pleg->buf) >= need_bytes) {
                        ready = true;
                        break;
                    }
                }
                mtx_unlock(&g_mx_lock);
                if (ready || !ptime)
                    break;
                uint64_t now_wait = tmr_jiffies();
                if (now_wait >= wait_deadline)
                    break;
                sys_msleep(1);
            }
        }

        memset(acc, 0, sizeof(int32_t) * alloc_sampc);
        uint32_t min_ms = 0;
        uint32_t max_ms = 0;
        bool mixed = false;

        if (g_mx_lock_ready)
            mtx_lock(&g_mx_lock);
        struct le *le = list_head(&g_mx_legs);
        while (le) {
            struct mx_leg_ctx *leg = le->data;
            le = le->next;
            if (!leg || !leg->buf)
                continue;

            if (!leg->play || !leg->play->primed)
                continue;

            size_t cur_before = aubuf_cur_size(leg->buf);
            if (cur_before < need_bytes) {
                g_mx_m.in_underrun5s++;
                continue;
            }

            struct auframe af;
            auframe_init(&af, AUFMT_S16LE, tmp, alloc_sampc, srate, ch);
            aubuf_read_auframe(leg->buf, &af);
            size_t cur_after = aubuf_cur_size(leg->buf);

            uint64_t abs_sum = 0;
            for (size_t i = 0; i < alloc_sampc; ++i) {
                int16_t v = tmp[i];
                abs_sum += (uint64_t)(v < 0 ? -v : v);
                acc[i] += (int32_t)((double)v * g_mx_gain_in);
            }
            bool silence = alloc_sampc ? (abs_sum / alloc_sampc) < 64 : true;
            if (!g_mx_first_in && !silence && alloc_sampc)
                g_mx_first_in = true;

            g_mx_m.in_frames5s++;
            g_mx_m.in_samples5s += alloc_sampc;
            if (silence)
                g_mx_m.in_silence5s++;

            if (bytes_per_ms && cur_after) {
                uint32_t ms = (uint32_t)(cur_after / bytes_per_ms);
                if (!min_ms || ms < min_ms) min_ms = ms;
                if (ms > max_ms) max_ms = ms;
            }

            mixed = true;
        }
        if (g_mx_lock_ready) {
            if (mixed) {
                g_mx_m.out_frames5s++;
                g_mx_m.out_samples5s += alloc_sampc;
                if (min_ms && (g_mx_m.bridge_ms_min == 0 || min_ms < g_mx_m.bridge_ms_min))
                    g_mx_m.bridge_ms_min = min_ms;
                if (max_ms > g_mx_m.bridge_ms_max)
                    g_mx_m.bridge_ms_max = max_ms;
            }
            mtx_unlock(&g_mx_lock);
        }

        bool tone_active = false;
        if (g_mx_gain_dtmf > 0.0 && g_mx_dtmf_len > 0 && srate) {
            char digit = g_mx_dtmf_seq[g_mx_dtmf_idx % g_mx_dtmf_len];
            if (g_mx_dtmf_elapsed_ms == 0)
                mx_dtmf_select_digit(digit, srate);
            uint32_t on_ms = (digit == '+') ? 0 :
                (g_mx_dtmf_period_ms > g_mx_dtmf_off_ms ?
                    g_mx_dtmf_period_ms - g_mx_dtmf_off_ms : g_mx_dtmf_period_ms);
            tone_active = (digit != '+') && g_mx_inc1 > 0.0 && g_mx_inc2 > 0.0 &&
                          (g_mx_dtmf_elapsed_ms < on_ms);
            if (tone_active)
                g_mx_m.tone_on5s++;
            for (size_t i = 0; i < alloc_sampc; ++i) {
                if (tone_active) {
                    double s = sin(g_mx_ph1) + sin(g_mx_ph2);
                    g_mx_ph1 += g_mx_inc1;
                    g_mx_ph2 += g_mx_inc2;
                    if (g_mx_ph1 > 2.0 * M_PI) g_mx_ph1 -= 2.0 * M_PI;
                    if (g_mx_ph2 > 2.0 * M_PI) g_mx_ph2 -= 2.0 * M_PI;
                    acc[i] += (int32_t)(s * (double)INT16_MAX * g_mx_gain_dtmf);
                }
            }
            g_mx_dtmf_elapsed_ms += ptime;
            uint32_t period = (digit == '+') ? g_mx_dtmf_pause_ms : g_mx_dtmf_period_ms;
            if (g_mx_dtmf_elapsed_ms >= period) {
                g_mx_dtmf_elapsed_ms = 0;
                g_mx_dtmf_idx = (g_mx_dtmf_idx + 1) % g_mx_dtmf_len;
                g_mx_ph1 = g_mx_ph2 = 0.0;
                g_mx_inc1 = g_mx_inc2 = 0.0;
            }
        }

        for (size_t i = 0; i < alloc_sampc; ++i) {
            int32_t v = acc[i];
            if (v > INT16_MAX) v = INT16_MAX;
            else if (v < INT16_MIN) v = INT16_MIN;
            mixv[i] = (int16_t)v;
        }

        struct auframe out;
        auframe_init(&out, AUFMT_S16LE, mixv, alloc_sampc, srate, ch);
        st->rh(&out, st->arg);

        next += ptime;
        uint64_t now = tmr_jiffies();
        if (next > now + 100)
            next = now + 100;
        if (next > now)
            sys_msleep((unsigned)(next - now));
    }

    mixv = mem_deref(mixv);
    tmp = mem_deref(tmp);
    acc = mem_deref(acc);
    return 0;
}

static void mx_src_destructor(void *arg)
{
    struct mx_src_st *st = arg;
    st->run = false;
    if (st->th)
        thrd_join(st->th, NULL);
}

static int mx_src_alloc(struct ausrc_st **stp, const struct ausrc *as,
                        struct ausrc_prm *prm, const char *device,
                        ausrc_read_h *rh, ausrc_error_h *errh, void *arg)
{
    (void)as; (void)device;
    if (!stp || !prm || !rh)
        return EINVAL;
    struct mx_src_st *st = mem_zalloc(sizeof(*st), mx_src_destructor);
    if (!st)
        return ENOMEM;
    st->prm = *prm;
    st->rh = rh;
    st->errh = errh;
    st->arg = arg;
    st->run = true;

    g_mx_srate = prm->srate ? prm->srate : g_mx_srate;
    if (!g_mx_srate) g_mx_srate = 8000;
    g_mx_ch = prm->ch ? prm->ch : g_mx_ch;
    if (!g_mx_ch) g_mx_ch = 1;
    g_mx_ptime = prm->ptime ? prm->ptime : g_mx_ptime;
    if (!g_mx_ptime) g_mx_ptime = 20;
    g_mx_sampc = (g_mx_srate * g_mx_ch * g_mx_ptime) / 1000;
    if (!g_mx_sampc) g_mx_sampc = 160;

    if (0 != thread_create_name(&st->th, "mx_src", mx_src_thread, st)) {
        mem_deref(st);
        return ENOMEM;
    }

    *stp = (struct ausrc_st *)st;
    return 0;
}

static void mx_leg_remove_all(void)
{
    if (!g_mx_lock_ready)
        return;
    for (;;) {
        mtx_lock(&g_mx_lock);
        struct le *le = list_head(&g_mx_legs);
        if (!le) {
            list_init(&g_mx_legs);
            mtx_unlock(&g_mx_lock);
            break;
        }
        list_unlink(le);
        struct mx_leg_ctx *leg = le->data;
        mtx_unlock(&g_mx_lock);
        mem_deref(leg);
    }
}

static void mx_metrics_tick(void *arg)
{
    (void)arg;
    if (!g_mx_lock_ready) {
        tmr_start(&g_mx_m_tmr, 5000, mx_metrics_tick, NULL);
        return;
    }

    uint32_t legs = 0;
    uint32_t in_frames = 0;
    uint32_t out_frames = 0;
    uint32_t tone_frames = 0;
    uint32_t silence_frames = 0;
    uint32_t underrun_frames = 0;
    uint64_t in_samples = 0;
    uint64_t out_samples = 0;
    uint32_t min_ms = 0;
    uint32_t max_ms = 0;
    uint32_t cur_ms = 0;

    mtx_lock(&g_mx_lock);
    legs = list_count(&g_mx_legs);
    in_frames = g_mx_m.in_frames5s;
    out_frames = g_mx_m.out_frames5s;
    tone_frames = g_mx_m.tone_on5s;
    silence_frames = g_mx_m.in_silence5s;
    underrun_frames = g_mx_m.in_underrun5s;
    in_samples = g_mx_m.in_samples5s;
    out_samples = g_mx_m.out_samples5s;
    min_ms = g_mx_m.bridge_ms_min;
    max_ms = g_mx_m.bridge_ms_max;

    size_t bytes_per_ms = (g_mx_srate * g_mx_ch * 2) / 1000;
    if (bytes_per_ms) {
        for (struct le *le = list_head(&g_mx_legs); le; le = le->next) {
            struct mx_leg_ctx *leg = le->data;
            if (!leg || !leg->buf)
                continue;
            size_t cur = aubuf_cur_size(leg->buf);
            uint32_t ms = (uint32_t)(cur / bytes_per_ms);
            if (ms > cur_ms)
                cur_ms = ms;
        }
    }

    g_mx_m.in_frames5s = 0;
    g_mx_m.out_frames5s = 0;
    g_mx_m.tone_on5s = 0;
    g_mx_m.in_silence5s = 0;
    g_mx_m.in_underrun5s = 0;
    g_mx_m.in_samples5s = 0;
    g_mx_m.out_samples5s = 0;
    g_mx_m.bridge_ms_min = 0;
    g_mx_m.bridge_ms_max = 0;
    mtx_unlock(&g_mx_lock);

    if (g_role && strcmp(g_role, "MIX ") == 0) {
        re_printf("MIX_METRICS5s legs=%u in_frames=%u out_frames=%u in_samples=%" PRIu64
                  " out_samples=%" PRIu64 " tone_on=%u silence_in=%u underrun=%u "
                  "bridge_ms=%u(%u..%u)\n",
                  legs,
                  in_frames,
                  out_frames,
                  in_samples,
                  out_samples,
                  tone_frames,
                  silence_frames,
                  underrun_frames,
                  cur_ms,
                  min_ms,
                  max_ms);
        fflush(NULL);
    }

    tmr_start(&g_mx_m_tmr, 5000, mx_metrics_tick, NULL);
}

int sip_mixer_init(const char* bind_addr, const char* target_uri,
                   uint32_t srate, uint8_t ch, uint32_t ptime_ms)
{
    int err = 0;
    g_role = "MIX ";
    g_log.h = log_adapter;
    log_register_handler(&g_log);
    log_enable_stdout(false);
    log_enable_timestamps(false);
    log_enable_color(false);
    log_enable_info(false);

    g_mx_srate = srate ? srate : g_mx_srate;
    if (!g_mx_srate) g_mx_srate = 8000;
    g_mx_ch = ch ? ch : g_mx_ch;
    if (!g_mx_ch) g_mx_ch = 1;
    g_mx_ptime = ptime_ms ? ptime_ms : g_mx_ptime;
    if (!g_mx_ptime) g_mx_ptime = 20;
    g_mx_sampc = (g_mx_srate * g_mx_ch * g_mx_ptime) / 1000;
    if (!g_mx_sampc) g_mx_sampc = 160;

    mx_lock_init();

    if (!g_mx_play_registered) {
        err = auplay_register(&g_mx_play, baresip_auplayl(), "b2b_mix", mx_play_alloc);
        if (err)
            return err;
        g_mx_play_registered = true;
    }

    err |= configure(bind_addr);
    const char *play_cfg = "audio_player\tb2b_mix,inbound\n";
    err |= conf_configure_buf((const uint8_t *)play_cfg, strlen(play_cfg));

    if (!g_mx_src) {
        err |= ausrc_register(&g_mx_src, baresip_ausrcl(), "b2b_mix_src", mx_src_alloc);
        if (err)
            return err;
    }

    if (!g_mx_in) {
        err |= ua_alloc(&g_mx_in, "sip:anon@0.0.0.0;regint=0;catchall=yes;audio_codecs=pcmu;audio_player=b2b_mix,inbound");
        if (err)
            return err;
        (void)ua_set_autoanswer_value(g_mx_in, "yes");
        ua_set_catchall(g_mx_in, true);
    }
    g_ua = g_mx_in;
    tmr_start(&g_aa_tmr, 50, aa_tick, NULL);

    if (!g_mx_out) {
        err |= ua_alloc(&g_mx_out, "sip:anon@0.0.0.0;regint=0;audio_codecs=pcmu");
        if (err)
            return err;
    }
    if (target_uri && *target_uri) {
        char cfg[256];
        re_snprintf(cfg, sizeof(cfg),
                    "audio_source\tb2b_mix_src,outbound\n"
                    "ausrc_srate\t%u\n"
                    "ausrc_channels\t%u\n"
                    "ausrc_format\ts16\n",
                    g_mx_srate, g_mx_ch);
        err |= conf_configure_buf((const uint8_t *)cfg, strlen(cfg));

        struct call *call = NULL;
        char from[128] = {0};
        const char *p = target_uri;
        if (0 == strncmp(p, "sip:", 4))
            p += 4;
        const char *host = p;
        const char *at = strchr(host, '@');
        if (at)
            host = at + 1;
        const char *last_colon = strrchr(host, ':');
        size_t host_len = last_colon ? (size_t)(last_colon - host) : strlen(host);
        if (host_len > sizeof(from) - 16)
            host_len = sizeof(from) - 16;
        memcpy(from, "sip:anon@", 9);
        memcpy(from + 9, host, host_len);
        from[9 + host_len] = '\0';
        err |= ua_connect(g_mx_out, &call, from, target_uri, VIDMODE_OFF);
    }

    g_mx_m.in_frames5s = 0;
    g_mx_m.out_frames5s = 0;
    g_mx_m.tone_on5s = 0;
    g_mx_m.in_silence5s = 0;
    g_mx_m.in_underrun5s = 0;
    g_mx_m.in_samples5s = 0;
    g_mx_m.out_samples5s = 0;
    g_mx_m.bridge_ms_min = 0;
    g_mx_m.bridge_ms_max = 0;
    g_mx_first_in = false;
    g_mx_dtmf_idx = 0;
    g_mx_dtmf_elapsed_ms = 0;

    tmr_start(&g_mx_m_tmr, 5000, mx_metrics_tick, NULL);

    return err;
}

int sip_mixer_shutdown(void)
{
    tmr_cancel(&g_aa_tmr);
    tmr_cancel(&g_mx_m_tmr);
    if (g_mx_out) {
        ua_destroy(g_mx_out);
        g_mx_out = NULL;
    }
    if (g_mx_in) {
        ua_destroy(g_mx_in);
        g_mx_in = NULL;
    }
    mx_leg_remove_all();
    if (g_mx_src) {
        mem_deref(g_mx_src);
        g_mx_src = NULL;
    }
    g_mx_first_in = false;
    g_mx_dtmf_idx = 0;
    g_mx_dtmf_elapsed_ms = 0;
    return 0;
}

int sip_mixer_config(const char* seq, uint32_t period_ms, float gain_in, float gain_dtmf)
{
    if (seq && *seq) {
        size_t n = strlen(seq);
        if (n >= sizeof(g_mx_dtmf_buf))
            n = sizeof(g_mx_dtmf_buf) - 1;
        memcpy(g_mx_dtmf_buf, seq, n);
        g_mx_dtmf_buf[n] = '\0';
        g_mx_dtmf_seq = g_mx_dtmf_buf;
        g_mx_dtmf_len = n ? n : 1;
    }
    if (period_ms)
        g_mx_dtmf_period_ms = period_ms;
    if (gain_in < 0.0f) gain_in = 0.0f;
    if (gain_in > 1.0f) gain_in = 1.0f;
    if (gain_dtmf < 0.0f) gain_dtmf = 0.0f;
    if (gain_dtmf > 1.0f) gain_dtmf = 1.0f;
    g_mx_gain_in = gain_in;
    g_mx_gain_dtmf = gain_dtmf;
    g_mx_dtmf_idx = 0;
    g_mx_dtmf_elapsed_ms = 0;
    g_mx_ph1 = g_mx_ph2 = 0.0;
    g_mx_inc1 = g_mx_inc2 = 0.0;
    return 0;
}
// sink metrics counters
static struct { uint32_t frames1s; uint32_t drops1s; } g_sink_m;

void sink_metrics_count_frame(void) { g_sink_m.frames1s++; }
void sink_metrics_count_drop(void) { g_sink_m.drops1s++; }

static void sink_metrics_tick(void *arg)
{
    (void)arg;
    if (g_role && strcmp(g_role, "SINK") == 0) {
        re_printf("SINK_METRICS5s rx_frames=%u drops=%u\n", g_sink_m.frames1s, g_sink_m.drops1s);
        fflush(NULL);
        g_sink_m.frames1s = 0; g_sink_m.drops1s = 0;
    }
    tmr_start(&g_sink_m_tmr, 5000, sink_metrics_tick, NULL);
}
