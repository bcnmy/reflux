#!/usr/bin/env bash

GIT_ROOT=$(git rev-parse --show-toplevel)
SCRIPT_DIR="${GIT_ROOT}"/k8s

IMAGE_TAG=$1

if [[ -z "${IMAGE_TAG}" ]] ; then
  IMAGE_TAG=latest
fi

time helm upgrade reflux "${SCRIPT_DIR}" \
     --install \
     --wait \
     --atomic \
     --values "${SCRIPT_DIR}/values.prod.yaml" \
     --set-string namespace=reflux \
     --set image_tag="${IMAGE_TAG}" \
     --namespace reflux
