// SPDX-License-Identifier: GPL-3.0-or-later

// The fixture's own declarations of the parts of SDRplay's API 3.15 that
// sdrtop uses, written from SDRplay's published specification. The layout
// tests compile a probe against this file and against the installed API's
// headers, and both must agree with the Rust structs.
#ifndef SDRTOP_FIXTURE_SDRPLAY_H
#define SDRTOP_FIXTURE_SDRPLAY_H

#define SDRPLAY_API_VERSION (float)(3.15)
#define SDRPLAY_RSP1B_ID (6)

typedef struct {
    char SerNo[64];
    unsigned char hwVer;
    int tuner;
    int rspDuoMode;
    unsigned char valid;
    double rspDuoSampleFreq;
    void *dev;
} sdrplay_api_DeviceT;

typedef struct { double fsHz; unsigned char syncUpdate; unsigned char reCal; } sdrplay_api_FsFreqT;
typedef struct { unsigned int sampleNum; unsigned int period; } sdrplay_api_SyncUpdateT;
typedef struct { unsigned char resetGainUpdate, resetRfUpdate, resetFsUpdate; } sdrplay_api_ResetFlagsT;
typedef struct { unsigned char rfNotchEnable; unsigned char rfDabNotchEnable; } sdrplay_api_Rsp1aParamsT;
typedef struct { unsigned char extRefOutputEn; } sdrplay_api_Rsp2ParamsT;
typedef struct { int extRefOutputEn; } sdrplay_api_RspDuoParamsT;
typedef struct {
    unsigned char hdrEnable; unsigned char biasTEnable; int antennaSel;
    unsigned char rfNotchEnable; unsigned char rfDabNotchEnable;
} sdrplay_api_RspDxParamsT;
typedef struct {
    double ppm;
    sdrplay_api_FsFreqT fsFreq;
    sdrplay_api_SyncUpdateT syncUpdate;
    sdrplay_api_ResetFlagsT resetFlags;
    int mode;
    unsigned int samplesPerPkt;
    sdrplay_api_Rsp1aParamsT rsp1aParams;
    sdrplay_api_Rsp2ParamsT rsp2Params;
    sdrplay_api_RspDuoParamsT rspDuoParams;
    sdrplay_api_RspDxParamsT rspDxParams;
} sdrplay_api_DevParamsT;

typedef struct { float curr; float max; float min; } sdrplay_api_GainValuesT;
typedef struct {
    int gRdB; unsigned char LNAstate; unsigned char syncUpdate; int minGr;
    sdrplay_api_GainValuesT gainVals;
} sdrplay_api_GainT;
typedef struct { double rfHz; unsigned char syncUpdate; } sdrplay_api_RfFreqT;
typedef struct { unsigned char dcCal; unsigned char speedUp; int trackTime; int refreshRateTime; } sdrplay_api_DcOffsetTunerT;
typedef struct {
    int bwType; int ifType; int loMode;
    sdrplay_api_GainT gain;
    sdrplay_api_RfFreqT rfFreq;
    sdrplay_api_DcOffsetTunerT dcOffsetTuner;
} sdrplay_api_TunerParamsT;
typedef struct { unsigned char DCenable; unsigned char IQenable; } sdrplay_api_DcOffsetT;
typedef struct { unsigned char enable; unsigned char decimationFactor; unsigned char wideBandSignal; } sdrplay_api_DecimationT;
typedef struct {
    int enable; int setPoint_dBfs;
    unsigned short attack_ms, decay_ms, decay_delay_ms, decay_threshold_dB;
    int syncUpdate;
} sdrplay_api_AgcT;
typedef struct {
    sdrplay_api_DcOffsetT dcOffset;
    sdrplay_api_DecimationT decimation;
    sdrplay_api_AgcT agc;
    int adsbMode;
} sdrplay_api_ControlParamsT;
typedef struct { unsigned char biasTEnable; } sdrplay_api_Rsp1aTunerParamsT;
typedef struct { unsigned char biasTEnable; int amPortSel; int antennaSel; unsigned char rfNotchEnable; } sdrplay_api_Rsp2TunerParamsT;
typedef struct { unsigned char resetGainUpdate; unsigned char resetRfUpdate; } sdrplay_api_RspDuo_ResetSlaveFlagsT;
typedef struct {
    unsigned char biasTEnable; int tuner1AmPortSel; unsigned char tuner1AmNotchEnable;
    unsigned char rfNotchEnable; unsigned char rfDabNotchEnable;
    sdrplay_api_RspDuo_ResetSlaveFlagsT resetSlaveFlags;
} sdrplay_api_RspDuoTunerParamsT;
typedef struct { int hdrBw; } sdrplay_api_RspDxTunerParamsT;
typedef struct {
    sdrplay_api_TunerParamsT tunerParams;
    sdrplay_api_ControlParamsT ctrlParams;
    sdrplay_api_Rsp1aTunerParamsT rsp1aTunerParams;
    sdrplay_api_Rsp2TunerParamsT rsp2TunerParams;
    sdrplay_api_RspDuoTunerParamsT rspDuoTunerParams;
    sdrplay_api_RspDxTunerParamsT rspDxTunerParams;
} sdrplay_api_RxChannelParamsT;
typedef struct {
    sdrplay_api_DevParamsT *devParams;
    sdrplay_api_RxChannelParamsT *rxChannelA;
    sdrplay_api_RxChannelParamsT *rxChannelB;
} sdrplay_api_DeviceParamsT;

typedef struct {
    unsigned int firstSampleNum; int grChanged; int rfChanged; int fsChanged; unsigned int numSamples;
} sdrplay_api_StreamCbParamsT;
typedef struct { unsigned int gRdB; unsigned int lnaGRdB; double currGain; } sdrplay_api_GainCbParamT;
typedef struct { int powerOverloadChangeType; } sdrplay_api_PowerOverloadCbParamT;
typedef struct { int modeChangeType; } sdrplay_api_RspDuoModeCbParamT;
typedef union {
    sdrplay_api_GainCbParamT gainParams;
    sdrplay_api_PowerOverloadCbParamT powerOverloadParams;
    sdrplay_api_RspDuoModeCbParamT rspDuoModeParams;
} sdrplay_api_EventParamsT;
typedef void (*sdrplay_api_StreamCallback_t)(short *xi, short *xq, sdrplay_api_StreamCbParamsT *params,
                                             unsigned int numSamples, unsigned int reset, void *cbContext);
typedef void (*sdrplay_api_EventCallback_t)(int eventId, int tuner, sdrplay_api_EventParamsT *params, void *cbContext);
typedef struct {
    sdrplay_api_StreamCallback_t StreamACbFn;
    sdrplay_api_StreamCallback_t StreamBCbFn;
    sdrplay_api_EventCallback_t EventCbFn;
} sdrplay_api_CallbackFnsT;

#endif
