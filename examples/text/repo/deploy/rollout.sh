#!/bin/sh
kubectl rollout restart deployment/billing
billingctl status
