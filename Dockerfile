FROM alpine:3.20

# 换用国内阿里云镜像源加速 apk 安装
RUN sed -i 's/dl-cdn.alpinelinux.org/mirrors.aliyun.com/g' /etc/apk/repositories \
    && apk add --no-cache sshpass openssh-client tzdata ca-certificates

# 复制已静态编译的 Linux 单文件二进制
COPY drzk-pay-sim-linux-x86_64 /usr/local/bin/drzk-pay-sim
RUN chmod +x /usr/local/bin/drzk-pay-sim

WORKDIR /data

# 默认端口
EXPOSE 18766

ENTRYPOINT ["/usr/local/bin/drzk-pay-sim"]
CMD ["--server", "192.168.65.58", "--no-open"]
