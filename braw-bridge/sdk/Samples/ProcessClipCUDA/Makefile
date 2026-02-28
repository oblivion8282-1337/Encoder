CXX=g++
CXXFLAGS=-g -std=c++11
SDKDIR=../../Include
INCLUDES=-I$(SDKDIR) -I$(CUDA_PATH)/include -L$(CUDA_PATH)/lib64/stubs

TARGETNAME=ProcessClipCUDA
LIBRARYNAME=BlackmagicRawAPIDispatch

all: $(TARGETNAME) cleanobj
.PHONY : all

debug: CXXFLAGS += -DDEBUG
debug: all

$(TARGETNAME) : $(TARGETNAME).o $(LIBRARYNAME).o
	$(CXX) $(CXXFLAGS) $(INCLUDES) -o $(TARGETNAME) $(TARGETNAME).o $(LIBRARYNAME).o -lpthread -ldl -lcuda

$(TARGETNAME).o : $(TARGETNAME).cpp $(SDKDIR)/$(LIBRARYNAME).cpp $(SDKDIR)/BlackmagicRawAPI.h $(SDKDIR)/LinuxCOM.h
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $(TARGETNAME).cpp

$(LIBRARYNAME).o : $(SDKDIR)/$(LIBRARYNAME).cpp $(SDKDIR)/BlackmagicRawAPI.h
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $(SDKDIR)/$(LIBRARYNAME).cpp

clean:
	$(RM) $(TARGETNAME) $(TARGETNAME).o $(LIBRARYNAME).o

cleanobj:
	$(RM) $(TARGETNAME).o $(LIBRARYNAME).o
