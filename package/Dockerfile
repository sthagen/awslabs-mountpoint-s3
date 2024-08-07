FROM public.ecr.aws/docker/library/centos:7

# Need this because of Centos 7 reached EOL on July 1, 2024 and mirrorlist.centos.org does not exist anymore.
RUN sed -i s/mirror.centos.org/vault.centos.org/g /etc/yum.repos.d/*.repo
RUN sed -i s/^#.*baseurl=http/baseurl=https/g /etc/yum.repos.d/*.repo
RUN sed -i s/^mirrorlist=http/#mirrorlist=https/g /etc/yum.repos.d/*.repo

RUN yum install -y epel-release centos-release-scl

# Fix up the newly added SCL repos, which don't have altarch baseurls
RUN sed -i s/mirror.centos.org/vault.centos.org/g /etc/yum.repos.d/*.repo
RUN sed -i s/^#.*baseurl=http/baseurl=https/g /etc/yum.repos.d/*.repo
RUN sed -i s/^mirrorlist=http/#mirrorlist=https/g /etc/yum.repos.d/*.repo
RUN if [ `uname -p` == "aarch64" ]; then sed -i s+centos/7+altarch/7+g /etc/yum.repos.d/*.repo; fi

RUN yum install -y \
        fuse \
        fuse-devel \
        make \
        cmake3 \
        git \
        pkgconfig \
        dpkg \
        fakeroot \
        rpmdevtools \
        tar \
        python3 \
        python3-pip \
        wget \
        devtoolset-10-gcc \
        devtoolset-10-gcc-c++ \
        llvm-toolset-7.0-clang \
        && \
    yum clean all

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y

RUN wget -q "https://github.com/EmbarkStudios/cargo-about/releases/download/0.6.1/cargo-about-0.6.1-$(uname -p)-unknown-linux-musl.tar.gz" && \
    wget -q "https://github.com/EmbarkStudios/cargo-about/releases/download/0.6.1/cargo-about-0.6.1-$(uname -p)-unknown-linux-musl.tar.gz.sha256" && \
    echo -n "$(sha256sum cargo-about-*.tar.gz | cut -d' ' -f1)" > checksum.sha256 && \
    diff cargo-about-*.tar.gz.sha256 checksum.sha256 && \
    tar xzf cargo-about-*.tar.gz && \
    cp cargo-about-*/cargo-about /usr/bin && \
    rm -rf cargo-about-* checksum.sha256

RUN python3 -m pip install dataclasses

ENV PATH="/opt/rh/llvm-toolset-7.0/root/usr/bin:/opt/rh/devtoolset-10/root/usr/bin:/root/.cargo/bin:$PATH"
ENV LD_LIBRARY_PATH="/opt/rh/llvm-toolset-7.0/root/usr/lib64:/opt/rh/devtoolset-10/root/usr/lib64:/opt/rh/devtoolset-10/root/usr/lib"
ENV CC="/opt/rh/devtoolset-10/root/usr/bin/gcc"
ENV CXX="/opt/rh/devtoolset-10/root/usr/bin/g++"

WORKDIR /mountpoint
ENTRYPOINT ["/mountpoint/package/package.py"]