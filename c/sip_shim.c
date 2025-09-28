// Minimal baresip integration layer for Sink role
#include <stdint.h>
#include <stddef.h>
#include <string.h>
#include <re.h>
#include <rem.h>
#include <baresip.h>
#include <stdio.h>

typedef void (*b2b_pcm_cb)(const int16_t* samples, size_t nsamples, void* user);

static b2b_pcm_cb g_cb = 0;
static void* g_user = 0;
static struct ua* g_ua = NULL;
static struct aufilt g_tap;
static struct tmr g_aa_tmr;
static struct log g_log;
static const char *g_role = NULL;
static struct ausrc *g_src = NULL;
static struct aubuf *g_src_ab = NULL;
static uint32_t g_src_srate = 8000;
static uint8_t  g_src_ch = 1;
static uint32_t g_src_ptime = 20;
static size_t   g_src_sampc = 160; // 20ms @ 8kHz mono
static volatile bool g_src_started = false;

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
                            "call_accept\tyes\n"
                            // Smoother playout on sink: larger adaptive buffer
                            "audio_buffer\t80-200\n"
                            "audio_buffer_mode\tadaptive\n"
                            // And adaptive RTP jitter buffer (pre-decode)
                            "audio_jitter_buffer_type\tadaptive\n"
                            "audio_jitter_buffer_ms\t80-160\n",
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
    return err;
}

int sip_sink_set_pcm_callback(b2b_pcm_cb cb, void* user) { g_cb = cb; g_user = user; return 0; }

int sip_sink_shutdown(void)
{
    if (g_ua) { ua_destroy(g_ua); g_ua = NULL; }
    tmr_cancel(&g_aa_tmr);
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
