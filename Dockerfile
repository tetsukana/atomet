FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt update && apt upgrade -y && \
    apt install -y \
        build-essential \
        git \
        wget \
        curl \
        unzip \
        rsync \
        bc \
        cpio \
        python3 \
        python3-dev \
        libssl-dev \
        libelf-dev \
        flex \
        bison \
        file \
        zip \
        lzop \
        gawk \
        texinfo \
        help2man \
        libtool \
        libtool-bin \
        cmake \
        autoconf \
        automake \
        gettext \
        libncurses5-dev \
        libncursesw5-dev \
        nano \
        vim \
        sudo \
        locales && \
    locale-gen en_US.UTF-8 && \
    apt clean && rm -rf /var/lib/apt/lists/* && \
    curl -fsSL https://deb.nodesource.com/setup_22.x | bash - && \
    apt install -y nodejs && \
    apt clean && rm -rf /var/lib/apt/lists/*

ENV LANG=en_US.UTF-8

WORKDIR /buildroot-dist
RUN curl -o - https://buildroot.org/downloads/buildroot-2024.02.tar.gz | tar zxvf -

RUN git clone --depth=1 https://github.com/Dafang-Hacks/mips-gcc472-glibc216-64bit.git /opt/mipsel-gcc472-glibc216 && \
    chmod +x /opt/mipsel-gcc472-glibc216/bin/*

WORKDIR /src

COPY buildscripts/docker_build.sh /usr/local/bin/docker_build
COPY buildscripts/rust_build.sh /usr/local/bin/rust_build
COPY buildscripts/imgproc_build.sh /usr/local/bin/imgproc_build
COPY buildscripts/web_build.sh /usr/local/bin/web_build
COPY buildscripts/drivers_build.sh /usr/local/bin/drivers_build
RUN chmod +x /usr/local/bin/docker_build /usr/local/bin/rust_build /usr/local/bin/imgproc_build /usr/local/bin/web_build /usr/local/bin/drivers_build

CMD ["/usr/local/bin/docker_build"]
