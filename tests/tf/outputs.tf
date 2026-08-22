output "region" {
  description = "Region everything lives in (teardown.sh sweeps here)."
  value       = var.region
}

output "public_ip" {
  description = "Public IP of the fuzz box."
  value       = aws_instance.fuzz.public_ip
}

output "ssh" {
  description = "SSH straight in (default Debian user is admin)."
  value       = "ssh admin@${aws_instance.fuzz.public_ip}"
}

output "run_fuzz" {
  description = "Deploy the version-locked tree and run the 10k gate."
  value       = "./run-fuzz.sh ${aws_instance.fuzz.public_ip}"
}

output "ready_check" {
  description = "cloud-init finishes asynchronously; wait for this before run-fuzz.sh."
  value       = "ssh admin@${aws_instance.fuzz.public_ip} 'cloud-init status --wait'"
}
