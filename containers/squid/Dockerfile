FROM alpine:3.21
RUN apk add --no-cache squid
EXPOSE 3128
CMD ["squid", "-N", "-f", "/etc/squid/squid.conf"]
