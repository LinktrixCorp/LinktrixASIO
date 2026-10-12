// Verifies the Rust DLL using the unmodified official Steinberg C++ interface.
#include <windows.h>
#include <unknwn.h>
#include <cstdio>
#include "iasiodrv.h"
#define CHECK(x) do {if(!(x)){printf("FAIL line %d: %s\n",__LINE__,#x);return 1;}} while(0)
static void bufferSwitch(long,ASIOBool) {}
int wmain(int argc,wchar_t** argv) {
    CHECK(argc==2);HMODULE dll=LoadLibraryW(argv[1]);CHECK(dll);
    auto get=reinterpret_cast<HRESULT(WINAPI*)(REFCLSID,REFIID,void**)>(GetProcAddress(dll,"DllGetClassObject"));CHECK(get);
    auto unload=reinterpret_cast<HRESULT(WINAPI*)()>(GetProcAddress(dll,"DllCanUnloadNow"));CHECK(unload);
    GUID clsid={0xa874e189,0x2361,0x4c9e,{0xae,0x64,0x56,0xa4,0x74,0x98,0x64,0x02}};
    IClassFactory* factory=nullptr;CHECK(SUCCEEDED(get(clsid,IID_IClassFactory,reinterpret_cast<void**>(&factory))));
    IASIO* asio=nullptr;CHECK(SUCCEEDED(factory->CreateInstance(nullptr,clsid,reinterpret_cast<void**>(&asio))));factory->Release();
    CHECK(asio->init(nullptr));char name[32]={};asio->getDriverName(name);printf("Driver: %s\n",name);
    long in=0,out=0;CHECK(asio->getChannels(&in,&out)==ASE_OK);CHECK(in==64&&out==64);
    CHECK(asio->canSampleRate(48000)==ASE_OK);CHECK(asio->canSampleRate(44100)==ASE_NoClock);
    long min=0,max=0,pref=0,gran=0;CHECK(asio->getBufferSize(&min,&max,&pref,&gran)==ASE_OK);CHECK(min==32&&max==2048&&pref==128&&gran==-1);
    ASIOCallbacks callbacks={};callbacks.bufferSwitch=bufferSwitch;
    ASIOBufferInfo buffers[128]={};for(long c=0;c<128;c++){buffers[c].isInput=c<64?ASIOTrue:ASIOFalse;buffers[c].channelNum=c%64;}
    CHECK(asio->createBuffers(buffers,128,128,&callbacks)==ASE_OK);
    for(long c=0;c<128;c++){ASIOChannelInfo info={};info.channel=c%64;info.isInput=c<64?ASIOTrue:ASIOFalse;CHECK(asio->getChannelInfo(&info)==ASE_OK);CHECK(info.isActive&&info.type==ASIOSTInt24LSB);CHECK(buffers[c].buffers[0]&&buffers[c].buffers[1]);}
    CHECK(asio->disposeBuffers()==ASE_OK);asio->Release();CHECK(unload()==S_OK);
    FreeLibrary(dll);printf("PASS: official SDK COM ABI, 64 inputs + 64 outputs, 24-bit / 48 kHz, all buffer pointers.\n");return 0;
}
