FROM alpine:3.20

RUN apk add --no-cache busybox-extras \
    && mkdir -p /srv/www

EXPOSE 8080
CMD ["busybox-extras", "httpd", "-f", "-p", "8080", "-h", "/srv/www"]
