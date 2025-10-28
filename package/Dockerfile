FROM alpine:3.22

# Used by quark subprocess.
RUN addgroup -S quark && adduser -S quark -G quark

WORKDIR /quark

COPY quark /usr/local/bin/quark

RUN mkdir -p \
      /var/log/quark \
      /quark/static \
      /etc/ssl \
      /run/quark \
      /etc/quark && \
    chown -R quark:quark \
      /quark/static

EXPOSE 80 443
ENTRYPOINT ["/usr/local/bin/quark"]
