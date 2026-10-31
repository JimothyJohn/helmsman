# OpENer POSIX sample as an interop target. Pinned to a commit so the test
# oracle doesn't move under us; bump deliberately.
# Build per OpENer bin/posix/setup_posix.sh; binary lands at
# bin/posix/src/ports/POSIX/OpENer (same path OpENer's own README uses).
FROM ubuntu:24.04 AS build
ARG OPENER_REF=0e1ab7ee5c272950c0dc8daf7793008d7a518912
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential cmake git ca-certificates libcap-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
RUN git clone https://github.com/EIPStackGroup/OpENer.git . \
    && git checkout --detach "$OPENER_REF"
WORKDIR /src/bin/posix
RUN cmake -DCMAKE_C_COMPILER=gcc -DOpENer_PLATFORM:STRING="POSIX" \
          -DCMAKE_BUILD_TYPE:STRING="" -DBUILD_SHARED_LIBS:BOOL=OFF ../../source \
    && make -j"$(nproc)"

FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends libcap2 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/bin/posix/src/ports/POSIX/OpENer /OpENer
# 44818 tcp+udp: encapsulation and explicit messaging. 2222 udp: Class 0/1 I/O.
EXPOSE 44818/tcp 44818/udp 2222/udp
ENTRYPOINT ["/OpENer", "eth0"]
