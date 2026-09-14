FROM alpine:3.20

RUN apk add --no-cache busybox-extras=1.36.1-r31 \
    && mkdir -p /srv/www

EXPOSE 8080
CMD ["busybox-extras", "httpd", "-f", "-p", "8080", "-h", "/srv/www"]
